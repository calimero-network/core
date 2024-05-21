import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import { useParams } from "react-router-dom";
import { useNavigate } from "react-router-dom";
import { useRPC } from "../hooks/useNear";
import { Package, Release } from "./Applications";
import ApplicationDetailsTable from "../components/applications/details/ApplicationDetailsTable";

export interface AppDetails {
  package: Package;
  releases: Release[];
}

export default function ApplicationDetails() {
  const { id } = useParams();
  const navigate = useNavigate();
  const { getPackage, getReleases } = useRPC();
  const [applicationInformation, setApplicationInformation] = useState<AppDetails>();

  useEffect(() => {
    const fetchApplicationData = async () => {
      if (id) {
        const packageData = await getPackage(id);
        const versionData = await getReleases(id);
        setApplicationInformation({
            package: packageData,
            releases: versionData,
        })
      }
    };
    fetchApplicationData();
  }, []);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        {applicationInformation && <ApplicationDetailsTable
          applicationInformation={applicationInformation}
          navigateToApplicationList={() => navigate("/applications")}
          navigateToAddRelease={() => navigate(`/applications/${id}/add-release`)}
        />}
      </PageContentWrapper>
    </FlexLayout>
  );
}
