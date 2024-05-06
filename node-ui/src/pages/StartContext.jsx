import React, { useState } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import { useNavigate } from "react-router-dom";
import { ContentCard } from "../components/common/ConentCard";
import StartContextCard from "../components/context/startContext/StartContextCard";
import translations from "../constants/en.global.json";

export default function StartContext() {
  const navigate = useNavigate();
  const [application, setApplication] = useState(null);
  const [isArgsChecked, setIsArgsChecked] = useState(false);
  const [methodName, setMethodName] = useState("");
  const [argumentsJson, setArgumentsJson] = useState("");
  const [showBrowseApplication, setShowBrowseApplication] = useState(false);
  const t = translations.startContextPage;

  const startContext = async () => {
    if (isArgsChecked) {
      //TODO add proper api call for starting context
      console.log(methodName);
      console.log(argumentsJson);
    } else {
      console.log(application);
    }
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
          />
        </ContentCard>
      </PageContentWrapper>
    </FlexLayout>
  );
}
