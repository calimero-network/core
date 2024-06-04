import React, { useState } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import { useNavigate } from "react-router-dom";
import { ContentCard } from "../components/common/ContentCard";
import StartContextCard from "../components/context/startContext/StartContextCard";
import translations from "../constants/en.global.json";
import apiClient from "../api/index";
import { useAdminClient } from "../hooks/useAdminClient";

export interface ContextApplication {
  appId: string;
  name: string;
  version: string;
}

export default function StartContext() {
  const t = translations.startContextPage;
  const navigate = useNavigate();
  const { installApplication } = useAdminClient();
  const [application, setApplication] = useState<ContextApplication>({
    appId: "",
    name: "",
    version: ""
  });
  const [isArgsChecked, setIsArgsChecked] = useState(false);
  const [methodName, setMethodName] = useState("");
  const [argumentsJson, setArgumentsJson] = useState("");
  const [showBrowseApplication, setShowBrowseApplication] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [startContextStatus, setStartContextStatus] = useState({
    title: "",
    message: "",
    error: false,
  });

  const startContext = async () => {
    setIsLoading(true);
    const response = await installApplicationHandler();
    if (!response) {
      setIsLoading(false);
      setShowStatusModal(true);
      return;
    }
    try {
      if (!application.appId) {
        return;
      }
      const startContextResponse = await apiClient
        .node()
        .startContexts(application.appId, methodName, argumentsJson);
      if (startContextResponse) {
        setStartContextStatus({
          title: t.startContextSuccessTitle,
          message: t.startedContextMessage,
          error: false,
        });
      } else {
        setStartContextStatus({
          title: t.startContextErrorTitle,
          message: t.startContextErrorMessage,
          error: true,
        });
      }
    } catch (error: any) {
      console.error(error);
      setStartContextStatus({
        title: t.startContextErrorTitle,
        message: error.message,
        error: true,
      });
    }
    setIsLoading(false);
    setShowStatusModal(true);
  };

  const installApplicationHandler = async (): Promise<boolean> => {
    if (!application.appId || !application.version) {
      return false;
    }
    const response = await installApplication(
      application.appId,
      application.version
    );
    if (response.error) {
      setStartContextStatus({
        title: "Error installing application",
        message: response.error.message,
        error: true,
      });
      return false;
    } else {
      setStartContextStatus({
        title: response.data,
        message: `Installed application ${application.name}, version ${application.version}.`,
        error: false,
      });
      return true;
    }
  };

  const closeModal = () => {
    setShowStatusModal(false);
    if (startContextStatus.error) {
      setStartContextStatus({
        title: "",
        message: "",
        error: false,
      });
      return;
    }
    navigate("/contexts");
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <ContentCard
          headerBackText={t.backButtonText}
          headerOnBackClick={() => navigate("/contexts")}
        >
          <StartContextCard
            application={application}
            setApplication={setApplication}
            isArgsChecked={isArgsChecked}
            setIsArgsChecked={setIsArgsChecked}
            methodName={methodName}
            setMethodName={setMethodName}
            argumentsJson={argumentsJson}
            setArgumentsJson={setArgumentsJson}
            startContext={startContext}
            showBrowseApplication={showBrowseApplication}
            setShowBrowseApplication={setShowBrowseApplication}
            onUploadClick={() => navigate("/publish-application")}
            isLoading={isLoading}
            showStatusModal={showStatusModal}
            closeModal={closeModal}
            startContextStatus={startContextStatus}
          />
        </ContentCard>
      </PageContentWrapper>
    </FlexLayout>
  );
}
