import React, { useEffect } from 'react';
import { Routes, Route, BrowserRouter } from 'react-router-dom';
import { setNodeUrlFromQuery } from './utils/storage';
import { useServerDown } from './context/ServerDownContext';

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
import Metamask from './pages/Metamask';
import Authenticate from './pages/Authenticate';
import AddRootKey from './pages/AddRootKey';
import SetupPage from './pages/setup';
import Near from './pages/Near';
import ProtectedRoute from './components/protectedRoutes/ProtectedRoute';
import NearRoute from './components/near/NearRoute';
import MetamaskRoute from './components/metamask/MetamaskRoute';
import InstallApplication from './pages/InstallApplication';
import StarknetLogin from './pages/Starknet';

import 'bootstrap/dist/css/bootstrap.min.css';

export default function App() {
  const { showServerDownPopup } = useServerDown();

  useEffect(() => {
    setNodeUrlFromQuery(showServerDownPopup);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <>
      <BrowserRouter basename="/admin-dashboard">
        <Routes>
          <Route path="/" element={<SetupPage />} />
          <Route element={<ProtectedRoute />}>
            <Route path="/auth" element={<Authenticate />} />
            <Route element={<NearRoute />}>
              <Route path="/auth/near" element={<Near isLogin={true} />} />
              <Route
                path="/identity/root-key/near"
                element={<Near isLogin={false} />}
              />
            </Route>
            <Route element={<MetamaskRoute />}>
              <Route
                path="/auth/metamask"
                element={<Metamask isLogin={true} />}
              />
              <Route
                path="/identity/root-key/metamask"
                element={<Metamask isLogin={false} />}
              />
            </Route>
            <Route
              path="/auth/starknet"
              element={<StarknetLogin isLogin={true} />}
            />
            <Route
              path="/identity/root-key/starknet"
              element={<StarknetLogin isLogin={false} />}
            />
            <Route path="/identity" element={<Identity />} />
            <Route path="/identity/root-key" element={<AddRootKey />} />
            <Route path="/applications" element={<ApplicationsPage />} />
            <Route
              path="/applications/install"
              element={<InstallApplication />}
            />
            <Route path="/applications/:id" element={<ApplicationDetails />} />
            <Route
              path="/publish-application"
              element={<PublishApplication />}
            />
            <Route
              path="/applications/:id/add-release"
              element={<AddRelease />}
            />
            <Route path="/contexts" element={<Contexts />} />
            <Route path="/contexts/start-context" element={<StartContext />} />
            <Route path="/contexts/join-context" element={<JoinContext />} />
            <Route path="/contexts/:id" element={<ContextDetails />} />
            <Route path="/export" element={<Export />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </>
  );
}
