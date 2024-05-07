import React, { useState } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import { useNavigate } from "react-router-dom";
import { ContentCard } from "../components/common/ConentCard";
import StartContextCard from "../components/context/startContext/StartContextCard";
import translations from "../constants/en.global.json";
import apiClient from "../api/index";

export default function StartContext() {
  const navigate = useNavigate();
  const [application, setApplication] = useState(null);
  const [isArgsChecked, setIsArgsChecked] = useState(false);
  const [methodName, setMethodName] = useState("");
  const [argumentsJson, setArgumentsJson] = useState("");
  const [showBrowseApplication, setShowBrowseApplication] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const t = translations.startContextPage;

  const startContext = async () => {
    setIsLoading(true);
    try {
      const startContextResponse = await apiClient
        .context()
        .startContexts(application.id, methodName, argumentsJson);
      console.log(startContextResponse);
    } catch (error) {
      console.error(error);
    }
    setIsLoading(false);
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
            onUploadClick={() => navigate("/upload-app")}
            isLoading={isLoading}
          />
        </ContentCard>
      </PageContentWrapper>
    </FlexLayout>
  );
}
