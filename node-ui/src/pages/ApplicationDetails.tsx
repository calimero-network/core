import React, { useState, useEffect } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import PageContentWrapper from '../components/common/PageContentWrapper';
import { useParams } from 'react-router-dom';
import { useNavigate } from 'react-router-dom';
import { useRPC } from '../hooks/useNear';
import { Package, Release } from './Applications';
import ApplicationDetailsTable from '../components/applications/details/ApplicationDetailsTable';
import { apiClient } from '@calimero-network/calimero-client';
import { AppMetadata, parseAppMetadata } from '../utils/metadata';

export interface AppDetails {
  package: Package;
  releases: Release[] | null;
}

export default function ApplicationDetailsPage() {
  const { id } = useParams();
  const navigate = useNavigate();
  const { getPackage, getReleases } = useRPC();
  const [applicationInformation, setApplicationInformation] =
    useState<AppDetails>();

  useEffect(() => {
    const fetchApplicationData = async () => {
      if (id) {
        const fetchApplicationDetailsResponse = await apiClient.node().getInstalledApplicationDetails(id);

        let appMetadata: AppMetadata | null = null;
        if (fetchApplicationDetailsResponse.error) {
          // dangerous: contract app id has different format than our app id so it returns bad request
          if (fetchApplicationDetailsResponse.error.code === 400) {
            //marketplace app
            appMetadata = {
              contractAppId: id,
            };
          } else {
            //Handle error
            console.error(fetchApplicationDetailsResponse.error.message);
          }
        } else {
          appMetadata = parseAppMetadata(
            fetchApplicationDetailsResponse.data.metadata,
          );
        }

        if (appMetadata) {
          const packageData = await getPackage(appMetadata.contractAppId);
          const versionData = await getReleases(appMetadata.contractAppId);
          if (packageData && versionData) {
            setApplicationInformation({
              package: packageData,
              releases: versionData,
            });
          }
        } else {
          setApplicationInformation({
            package: {
              id: id,
              name: 'Local app',
              description: '',
              repository: '',
              owner: '',
            },
            releases: null,
          });
        }
      }
    };
    fetchApplicationData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        {applicationInformation && (
          <ApplicationDetailsTable
            applicationInformation={applicationInformation}
            navigateToApplicationList={() => navigate('/applications')}
            navigateToAddRelease={() =>
              navigate(`/applications/${id}/add-release`)
            }
          />
        )}
      </PageContentWrapper>
    </FlexLayout>
  );
}
