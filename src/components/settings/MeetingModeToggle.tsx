import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen } from "lucide-react";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { commands } from "@/bindings";

export const MeetingModeToggle: React.FC = React.memo(() => {
  const { t } = useTranslation();
  const [supported, setSupported] = useState(false);
  const [active, setActive] = useState(false);
  const [loading, setLoading] = useState(false);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    commands.isMeetingModeSupported().then(setSupported);
    commands.getMeetingModeState().then(setActive);
    return () => {
      if (toastTimer.current) clearTimeout(toastTimer.current);
    };
  }, []);

  if (!supported) return null;

  const handleToggle = async (enabled: boolean) => {
    setLoading(true);
    setSavedPath(null);
    try {
      if (enabled) {
        await commands.startMeetingMode();
        setActive(true);
      } else {
        const result = await commands.stopMeetingMode();
        setActive(false);
        if (result.status === "ok" && result.data) {
          setSavedPath(result.data);
          toastTimer.current = setTimeout(() => setSavedPath(null), 6000);
        }
      }
    } catch (err) {
      console.error("Meeting mode toggle error:", err);
    }
    setLoading(false);
  };

  const handleOpenFolder = async () => {
    try {
      await commands.openMeetingsFolder();
    } catch (err) {
      console.error("Failed to open meetings folder:", err);
    }
  };

  return (
    <div>
      <ToggleSwitch
        checked={active}
        onChange={handleToggle}
        isUpdating={loading}
        label={t("settings.meetingMode.label")}
        description={t("settings.meetingMode.description")}
        descriptionMode="tooltip"
        grouped={true}
      />
      <div className="flex items-center gap-2 px-2 mt-1">
        <button
          onClick={handleOpenFolder}
          className="flex items-center gap-1 text-xs text-text/50 hover:text-text/80 transition-colors"
          title={t("settings.meetingMode.openFolder")}
        >
          <FolderOpen width={13} height={13} />
          <span>{t("settings.meetingMode.openFolder")}</span>
        </button>
        {savedPath && (
          <p className="text-xs text-green-600 truncate">
            {t("settings.meetingMode.transcriptSaved")}: {savedPath}
          </p>
        )}
      </div>
    </div>
  );
});
