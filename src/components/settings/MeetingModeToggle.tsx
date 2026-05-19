import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ToggleSwitch } from "../ui/ToggleSwitch";

export const MeetingModeToggle: React.FC = React.memo(() => {
  const { t } = useTranslation();
  const [supported, setSupported] = useState(false);
  const [active, setActive] = useState(false);
  const [loading, setLoading] = useState(false);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    invoke<boolean>("is_meeting_mode_supported").then(setSupported);
    invoke<boolean>("get_meeting_mode_state").then(setActive);
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
        await invoke("start_meeting_mode");
        setActive(true);
      } else {
        const filePath = await invoke<string | null>("stop_meeting_mode");
        setActive(false);
        if (filePath) {
          setSavedPath(filePath);
          toastTimer.current = setTimeout(() => setSavedPath(null), 6000);
        }
      }
    } catch (err) {
      console.error("Meeting mode toggle error:", err);
    }
    setLoading(false);
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
      {savedPath && (
        <p className="text-xs text-green-600 mt-1 px-2 truncate">
          {t("settings.meetingMode.transcriptSaved")}: {savedPath}
        </p>
      )}
    </div>
  );
});
