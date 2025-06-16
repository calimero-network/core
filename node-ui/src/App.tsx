import React from 'react';
import { Routes, Route, BrowserRouter, Navigate } from 'react-router-dom';
import { ProtectedRoutesWrapper } from '@calimero-network/calimero-client';

import Identity from './pages/Identity';
import ApplicationsPage from './pages/Applications';
import Contexts from './pages/Contexts';
import StartContext from './pages/StartContext';
import JoinContext from './pages/JoinContext';
import ContextDetails from './pages/ContextDetails';
import Export from './pages/Export';
import ApplicationDetails from './pages/ApplicationDetails';
import PublishApplication from './pages/PublishApplication';
import AddRelease from './pages/AddRelease';
import AddRootKey from './pages/AddRootKey';
import InstallApplication from './pages/InstallApplication';
import RootKeyProvidersWrapper from './components/keys/RootKeyProvidersWrapper';

import 'bootstrap/dist/css/bootstrap.min.css';

export default function App() {
  return (
    <BrowserRouter basename="/admin-dashboard">
      <ProtectedRoutesWrapper permissions={['admin']}>
        <Routes>
          {/* Redirect root to identity page */}
          <Route path="/" element={<Navigate to="/identity" replace />} />

          {/* Identity routes */}
          <Route path="/identity" element={<Identity />} />
          <Route path="/identity/root-key" element={<AddRootKey />} />
          <Route
            path="/identity/root-key/:providerId"
            element={<RootKeyProvidersWrapper />}
          />

          {/* Application routes */}
          <Route path="/applications" element={<ApplicationsPage />} />
          <Route
            path="/applications/install"
            element={<InstallApplication />}
          />
          <Route path="/applications/:id" element={<ApplicationDetails />} />
          <Route path="/publish-application" element={<PublishApplication />} />
          <Route
            path="/applications/:id/add-release"
            element={<AddRelease />}
          />

          {/* Context routes */}
          <Route path="/contexts" element={<Contexts />} />
          <Route path="/contexts/start-context" element={<StartContext />} />
          <Route path="/contexts/join-context" element={<JoinContext />} />
          <Route path="/contexts/:id" element={<ContextDetails />} />

          {/* Export route */}
          <Route path="/export" element={<Export />} />

          {/* Catch all unknown routes and redirect to identity */}
          <Route path="*" element={<Navigate to="/identity" replace />} />
        </Routes>
      </ProtectedRoutesWrapper>
    </BrowserRouter>
  );
}
