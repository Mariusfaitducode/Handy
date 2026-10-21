//! Notification texts. They are shown by macOS, outside the web view, so
//! they are written here, in the two languages meeting minutes support
//! (French for a French interface, English otherwise).

pub struct Strings {
    pub soon_title: fn(&str) -> String,
    pub soon_body_ask: &'static str,
    pub soon_body_auto: &'static str,
    pub ended_title: fn(&str) -> String,
    pub ended_body: &'static str,
    pub stopped_title: fn(&str) -> String,
    pub stopped_body: &'static str,
    pub busy_body: &'static str,
    pub record: &'static str,
    pub ignore: &'static str,
    pub dont_record: &'static str,
    pub stop: &'static str,
    pub keep: &'static str,
    /// Shown instead of an empty title.
    pub untitled: &'static str,
}

const FRENCH: Strings = Strings {
    soon_title: |t| format!("Réunion « {t} » dans 1 min"),
    soon_body_ask: "Enregistrer et transcrire cette réunion ?",
    soon_body_auto: "L'enregistrement démarrera automatiquement.",
    ended_title: |t| format!("« {t} » semble terminée"),
    ended_body: "Plus personne ne parle depuis quelques minutes. Arrêter l'enregistrement ?",
    stopped_title: |t| format!("Enregistrement de « {t} » arrêté"),
    stopped_body: "Personne n'a parlé depuis 20 minutes. La transcription est enregistrée.",
    busy_body: "Une réunion est déjà en cours d'enregistrement.",
    record: "Enregistrer",
    ignore: "Ignorer",
    dont_record: "Ne pas enregistrer",
    stop: "Arrêter",
    keep: "Continuer",
    untitled: "Sans titre",
};

const ENGLISH: Strings = Strings {
    soon_title: |t| format!("\u{201C}{t}\u{201D} starts in 1 min"),
    soon_body_ask: "Record and transcribe this meeting?",
    soon_body_auto: "Recording will start automatically.",
    ended_title: |t| format!("\u{201C}{t}\u{201D} seems to be over"),
    ended_body: "Nobody has spoken for a few minutes. Stop recording?",
    stopped_title: |t| format!("Stopped recording \u{201C}{t}\u{201D}"),
    stopped_body: "Nobody spoke for 20 minutes. The transcript is saved.",
    busy_body: "A meeting is already being recorded.",
    record: "Record",
    ignore: "Ignore",
    dont_record: "Don't record",
    stop: "Stop",
    keep: "Keep recording",
    untitled: "Untitled",
};

pub fn for_language(app_language: &str) -> &'static Strings {
    if app_language.to_lowercase().starts_with("fr") {
        &FRENCH
    } else {
        &ENGLISH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_interface_gets_french_prompts() {
        let fr = for_language("fr");
        assert_eq!(
            (fr.soon_title)("Point hebdo"),
            "Réunion « Point hebdo » dans 1 min"
        );
        assert_eq!(for_language("en").record, "Record");
        assert_eq!(for_language("de").record, "Record");
    }
}
