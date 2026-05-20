import React, { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { SettingContainer } from "../ui/SettingContainer";
import { PathDisplay } from "../ui/PathDisplay";

export const MeetingsFolderPath: React.FC = () => {
  const { t } = useTranslation();
  const [path, setPath] = useState<string>("");
  const [supported, setSupported] = useState(false);

  useEffect(() => {
    commands.getMeetingsDirPath().then((result) => {
      if (result.status === "ok") {
        setPath(result.data);
        setSupported(true);
      }
    });
  }, []);

  if (!supported) return null;

  const handleOpen = async () => {
    try {
      await commands.openMeetingsFolder();
    } catch (err) {
      console.error("Failed to open meetings folder:", err);
    }
  };

  return (
    <SettingContainer
      title={t("settings.meetingMode.directory.title")}
      description={t("settings.meetingMode.directory.description")}
      descriptionMode="inline"
      grouped={false}
      layout="stacked"
    >
      <PathDisplay path={path} onOpen={handleOpen} disabled={!path} />
    </SettingContainer>
  );
};
