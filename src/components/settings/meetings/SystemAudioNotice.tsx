import React from "react";
import { useTranslation } from "react-i18next";
import { commands, type SystemAudioState } from "@/bindings";

/** Why the other participants aren't transcribed, and the way out. */
export const SystemAudioNotice: React.FC<{ state: SystemAudioState }> = ({
  state,
}) => {
  const { t } = useTranslation();
  if (state === "captured") return null;
  return (
    <p className="text-xs text-text/70 leading-relaxed">
      {t(`meetings.systemAudio.${state}`)}
      {state === "denied" && (
        <>
          {" "}
          <button
            onClick={() => commands.openSystemAudioSettings()}
            className="underline underline-offset-2 hover:text-text cursor-pointer"
          >
            {t("meetings.systemAudio.openSettings")}
          </button>
        </>
      )}
    </p>
  );
};
