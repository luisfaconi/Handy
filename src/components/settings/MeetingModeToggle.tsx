import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { commands } from "@/bindings";

export const MeetingModeToggle: React.FC = React.memo(() => {
  const { t } = useTranslation();
  const [supported, setSupported] = useState(false);
  const [active, setActive] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    commands.isMeetingModeSupported().then(setSupported);
    commands.getMeetingModeState().then(setActive);
  }, []);

  if (!supported) return null;

  const handleToggle = async (enabled: boolean) => {
    setLoading(true);
    try {
      if (enabled) {
        await commands.startMeetingMode();
        setActive(true);
      } else {
        await commands.stopMeetingMode();
        setActive(false);
      }
    } catch (err) {
      console.error("Meeting mode toggle error:", err);
    }
    setLoading(false);
  };

  return (
    <ToggleSwitch
      checked={active}
      onChange={handleToggle}
      isUpdating={loading}
      label={t("settings.meetingMode.label")}
      description={t("settings.meetingMode.description")}
      descriptionMode="tooltip"
      grouped={true}
    />
  );
});
