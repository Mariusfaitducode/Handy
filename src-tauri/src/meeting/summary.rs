//! Meeting summaries through the language model configured for
//! post-processing (a cloud API, a local server, or Apple Intelligence).
//!
//! Nothing is sent anywhere unless post-processing is enabled and a provider
//! with a model is set up: the summary goes exactly where the user already
//! chose to send dictations, and the UI names that provider.

use log::{debug, warn};

use super::transcript::MeetingSegment;
use super::transcript::{format_duration, plain_transcript, split_for_model, Labels};
use crate::settings::{AppSettings, PostProcessProvider, APPLE_INTELLIGENCE_PROVIDER_ID};

/// Which language the summary and the speaker labels are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryLanguage {
    French,
    English,
}

impl SummaryLanguage {
    pub fn from_app_language(app_language: &str) -> Self {
        if app_language.to_lowercase().starts_with("fr") {
            Self::French
        } else {
            Self::English
        }
    }

    pub fn labels(self) -> Labels<'static> {
        match self {
            Self::French => Labels {
                me: "Moi",
                others: "Participants",
            },
            Self::English => Labels {
                me: "Me",
                others: "Participants",
            },
        }
    }
}

/// The model a summary would be written by.
#[derive(Debug, Clone)]
pub struct SummaryModel {
    pub provider: PostProcessProvider,
    pub model: String,
    pub api_key: String,
}

impl SummaryModel {
    /// "OpenAI · gpt-4o-mini", shown next to the summary.
    pub fn label(&self) -> String {
        if self.provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
            self.provider.label.clone()
        } else {
            format!("{} · {}", self.provider.label, self.model)
        }
    }

    fn is_apple(&self) -> bool {
        self.provider.id == APPLE_INTELLIGENCE_PROVIDER_ID
    }

    /// Largest transcript piece sent in one request, in characters. Apple's
    /// on-device model has a ~4k token window; local servers often default to
    /// small contexts and truncate silently; hosted models take far more.
    fn max_chunk_chars(&self) -> usize {
        match self.provider.id.as_str() {
            APPLE_INTELLIGENCE_PROVIDER_ID => 6_000,
            "custom" => 12_000,
            _ => 60_000,
        }
    }
}

/// The configured model, or `None` when summaries can't be written: post-
/// processing is off, or the active provider lacks a model or an API key.
pub fn resolve_model(settings: &AppSettings) -> Option<SummaryModel> {
    if !settings.post_process_enabled {
        return None;
    }
    let provider = settings.active_post_process_provider()?.clone();
    let model = settings
        .post_process_models
        .get(&provider.id)
        .map(|m| m.trim().to_string())
        .unwrap_or_default();
    if model.is_empty() {
        return None;
    }
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    let keyless = provider.id == APPLE_INTELLIGENCE_PROVIDER_ID || provider.id == "custom";
    if api_key.trim().is_empty() && !keyless {
        return None;
    }
    Some(SummaryModel {
        provider,
        model,
        api_key,
    })
}

fn system_prompt(language: SummaryLanguage) -> &'static str {
    match language {
        SummaryLanguage::French => {
            "Tu rédiges le compte rendu d'une réunion à partir de sa transcription automatique.\n\
             \n\
             La transcription vient d'une reconnaissance vocale : elle contient des fautes et des mots mal compris. \
             Corrige-les sans le signaler quand le sens est évident. N'invente jamais un fait, un chiffre, une date ou un nom.\n\
             \n\
             Les interlocuteurs sont « Moi » (la personne qui a enregistré la réunion) et « Participants » (toutes les autres \
             personnes, regroupées : ne les distingue que si un prénom est clairement mentionné).\n\
             \n\
             Écris en français, en Markdown, avec ces sections dans cet ordre :\n\
             \n\
             ## Résumé\n\
             Trois à six phrases : le contexte, ce qui a été discuté, où l'on en est arrivé.\n\
             \n\
             ## Décisions\n\
             Une puce par décision prise. S'il n'y en a pas : « Aucune décision explicite. »\n\
             \n\
             ## Actions\n\
             Une case par action : « - [ ] **Qui** : quoi (échéance si elle a été dite) ». « Qui » est « Moi », un prénom \
             cité, ou « À définir ». S'il n'y en a pas : « Aucune action identifiée. »\n\
             \n\
             ## Points ouverts\n\
             Les questions restées sans réponse et les sujets à reprendre. Omets cette section s'il n'y en a pas.\n\
             \n\
             Sois factuel et concis. Pas d'introduction ni de conclusion, ne recopie pas la transcription."
        }
        SummaryLanguage::English => {
            "You write the minutes of a meeting from its automatic transcript.\n\
             \n\
             The transcript comes from speech recognition: it contains errors and misheard words. Fix them silently when \
             the meaning is obvious. Never invent a fact, number, date or name.\n\
             \n\
             The speakers are \"Me\" (the person who recorded the meeting) and \"Participants\" (everyone else, grouped \
             together: only tell them apart when a first name is clearly mentioned).\n\
             \n\
             Write in English, in Markdown, with these sections in this order:\n\
             \n\
             ## Summary\n\
             Three to six sentences: the context, what was discussed, where things landed.\n\
             \n\
             ## Decisions\n\
             One bullet per decision made. If there are none: \"No explicit decision.\"\n\
             \n\
             ## Action items\n\
             One checkbox per action: \"- [ ] **Who**: what (deadline if one was said)\". \"Who\" is \"Me\", a first name \
             that was mentioned, or \"Unassigned\". If there are none: \"No action items.\"\n\
             \n\
             ## Open questions\n\
             Questions left unanswered and topics to revisit. Leave this section out if there are none.\n\
             \n\
             Be factual and concise. No introduction or conclusion, do not copy the transcript."
        }
    }
}

fn notes_prompt(language: SummaryLanguage, part: usize, parts: usize) -> String {
    match language {
        SummaryLanguage::French => format!(
            "Voici la partie {part} sur {parts} de la transcription automatique d'une réunion entre « Moi » et des \
             « Participants ». Prends des notes concises sous forme de puces : sujets abordés, décisions, actions \
             (qui, quoi, quand), questions ouvertes. Corrige les erreurs de reconnaissance évidentes, n'invente rien, \
             pas d'introduction."
        ),
        SummaryLanguage::English => format!(
            "This is part {part} of {parts} of the automatic transcript of a meeting between \"Me\" and \
             \"Participants\". Take concise bullet-point notes: topics discussed, decisions, action items (who, what, \
             when), open questions. Fix obvious recognition errors, invent nothing, no introduction."
        ),
    }
}

/// What the model is given about the meeting besides its transcript.
pub struct SummaryInput<'a> {
    pub title: Option<&'a str>,
    pub duration_ms: i64,
    pub segments: &'a [MeetingSegment],
    pub language: SummaryLanguage,
}

fn header(input: &SummaryInput<'_>) -> String {
    let (title, duration) = match input.language {
        SummaryLanguage::French => ("Titre", "Durée"),
        SummaryLanguage::English => ("Title", "Duration"),
    };
    let mut out = String::new();
    if let Some(t) = input.title.map(str::trim).filter(|t| !t.is_empty()) {
        out.push_str(&format!("{title} : {t}\n"));
    }
    out.push_str(&format!(
        "{duration} : {}\n",
        format_duration(input.duration_ms)
    ));
    out
}

/// Fewer words than this and there is nothing worth summarizing.
const MIN_WORDS: usize = 25;

pub fn has_enough_content(segments: &[MeetingSegment]) -> bool {
    segments
        .iter()
        .map(|s| s.text.split_whitespace().count())
        .sum::<usize>()
        >= MIN_WORDS
}

/// Writes the summary. Long transcripts are first condensed part by part,
/// then summarized from those notes.
pub async fn summarize(model: &SummaryModel, input: &SummaryInput<'_>) -> Result<String, String> {
    let transcript = plain_transcript(input.segments, input.language.labels());
    let system = system_prompt(input.language);
    let header = header(input);
    let budget = model
        .max_chunk_chars()
        .saturating_sub(header.len())
        .max(1_000);
    let pieces = split_for_model(&transcript, budget);

    let transcript_label = match input.language {
        SummaryLanguage::French => "Transcription",
        SummaryLanguage::English => "Transcript",
    };

    let body = if pieces.len() <= 1 {
        format!("{header}\n{transcript_label} :\n{transcript}")
    } else {
        debug!("Summarizing a long meeting in {} parts", pieces.len());
        let mut notes = Vec::with_capacity(pieces.len());
        for (i, piece) in pieces.iter().enumerate() {
            let prompt = notes_prompt(input.language, i + 1, pieces.len());
            let part = complete(model, &prompt, piece).await?;
            notes.push(format!(
                "### {} {}\n{}",
                transcript_label,
                i + 1,
                part.trim()
            ));
        }
        let notes_label = match input.language {
            SummaryLanguage::French => "Notes prises au fil de la réunion",
            SummaryLanguage::English => "Notes taken through the meeting",
        };
        let mut joined = notes.join("\n\n");
        // The notes must fit one request too; keep the start of each part if not.
        if joined.chars().count() > budget {
            let per_part = budget / notes.len();
            joined = notes
                .iter()
                .map(|n| n.chars().take(per_part).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        format!("{header}\n{notes_label} :\n\n{joined}")
    };

    let summary = complete(model, system, &body).await?;
    let summary = clean_output(&summary);
    if summary.is_empty() {
        return Err("The model returned an empty summary".to_string());
    }
    Ok(summary)
}

/// One request may take a while on a long transcript or a local model, but
/// never forever: a hung provider must not leave the minutes "pending".
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

async fn complete(model: &SummaryModel, system: &str, user: &str) -> Result<String, String> {
    match tokio::time::timeout(REQUEST_TIMEOUT, complete_once(model, system, user)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "No answer from {} after {} minutes",
            model.label(),
            REQUEST_TIMEOUT.as_secs() / 60
        )),
    }
}

async fn complete_once(model: &SummaryModel, system: &str, user: &str) -> Result<String, String> {
    if model.is_apple() {
        return complete_apple(system, user).await;
    }
    // Same reasoning shortcut post-processing uses for these providers.
    let disable_reasoning = matches!(model.provider.id.as_str(), "custom" | "openrouter");
    let prompt = format!("{system}\n\n---\n\n{user}");
    match crate::llm_client::send_chat_completion(
        &model.provider,
        model.api_key.clone(),
        &model.model,
        prompt,
        disable_reasoning,
    )
    .await
    {
        Ok(Some(content)) => Ok(content),
        Ok(None) => Err("The model returned no content".to_string()),
        Err(e) => {
            warn!("Meeting summary request failed: {e}");
            Err(e)
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
async fn complete_apple(system: &str, user: &str) -> Result<String, String> {
    let system = system.to_string();
    let user = user.to_string();
    tokio::task::spawn_blocking(move || {
        if !crate::apple_intelligence::check_apple_intelligence_availability() {
            return Err("Apple Intelligence is not available on this Mac".to_string());
        }
        crate::apple_intelligence::process_text_with_system_prompt(&system, &user, 0)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
async fn complete_apple(_system: &str, _user: &str) -> Result<String, String> {
    Err("Apple Intelligence is not available on this platform".to_string())
}

/// Strips what models wrap answers in: reasoning blocks, invisible
/// characters, a Markdown code fence around the whole answer.
pub fn clean_output(text: &str) -> String {
    let mut text = text
        .replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "")
        .trim()
        .to_string();
    if let Some(rest) = text.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            text = rest[end + "</think>".len()..].trim().to_string();
        }
    }
    if text.starts_with("```") && text.ends_with("```") && text.len() > 6 {
        let inner = &text[3..text.len() - 3];
        let inner = match inner.find('\n') {
            Some(newline) if !inner[..newline].contains(' ') => &inner[newline + 1..],
            _ => inner,
        };
        text = inner.trim().to_string();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::transcript::Speaker;
    use crate::settings::get_default_settings;

    #[test]
    fn language_follows_the_app_language() {
        assert_eq!(
            SummaryLanguage::from_app_language("fr-FR"),
            SummaryLanguage::French
        );
        assert_eq!(
            SummaryLanguage::from_app_language("en"),
            SummaryLanguage::English
        );
        assert_eq!(SummaryLanguage::French.labels().me, "Moi");
    }

    #[test]
    fn no_model_until_post_processing_is_set_up() {
        let mut settings = get_default_settings();
        assert!(resolve_model(&settings).is_none(), "disabled by default");

        settings.post_process_enabled = true;
        settings.post_process_provider_id = "openai".to_string();
        assert!(resolve_model(&settings).is_none(), "no model chosen");

        settings
            .post_process_models
            .insert("openai".to_string(), "gpt-4o-mini".to_string());
        assert!(resolve_model(&settings).is_none(), "no API key");

        settings
            .post_process_api_keys
            .insert("openai".to_string(), "sk-test".to_string());
        let model = resolve_model(&settings).unwrap();
        assert_eq!(model.label(), "OpenAI · gpt-4o-mini");
    }

    #[test]
    fn local_servers_need_no_key() {
        let mut settings = get_default_settings();
        settings.post_process_enabled = true;
        settings.post_process_provider_id = "custom".to_string();
        settings
            .post_process_models
            .insert("custom".to_string(), "llama3.2".to_string());
        assert!(resolve_model(&settings).is_some());
    }

    #[test]
    fn output_wrappers_are_removed() {
        assert_eq!(
            clean_output("```markdown\n## Résumé\nOK\n```"),
            "## Résumé\nOK"
        );
        assert_eq!(clean_output("<think>hmm</think>\n## Résumé"), "## Résumé");
        assert_eq!(clean_output("\u{FEFF}## Summary "), "## Summary");
        assert_eq!(clean_output("plain"), "plain");
    }

    #[test]
    fn tiny_meetings_are_not_summarized() {
        let seg = |text: &str| MeetingSegment {
            id: 0,
            speaker: Speaker::Me,
            start_ms: 0,
            end_ms: 0,
            text: text.to_string(),
        };
        assert!(!has_enough_content(&[seg("Test, un deux."), seg("Ok.")]));
        let long = "mot ".repeat(30);
        assert!(has_enough_content(&[seg(&long)]));
    }

    #[test]
    fn header_includes_title_and_duration() {
        let input = SummaryInput {
            title: Some("Point hebdo"),
            duration_ms: 30 * 60_000,
            segments: &[],
            language: SummaryLanguage::French,
        };
        assert_eq!(header(&input), "Titre : Point hebdo\nDurée : 30 min\n");
    }
}
