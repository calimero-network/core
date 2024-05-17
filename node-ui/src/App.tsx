import React from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Bootstrap from "./pages/Bootstrap";
import ConfirmWallet from "./pages/ConfirmWallet";
import Identity from "./pages/Identity";
import Applications from "./pages/Applications";
import Contexts from "./pages/Contexts";
import StartContext from "./pages/StartContext";
import ContextDetails from "./pages/ContextDetails";
import Export from "./pages/Export";
import ApplicationDetails from "./pages/ApplicationDetails";
import PublishApplication from "./pages/PublishApplication";

import "bootstrap/dist/css/bootstrap.min.css";

export default function App() {
  return (
    <>
      <BrowserRouter basename="/admin">
        <Routes>
          <Route path="/" element={<Bootstrap />} />
          <Route path="/confirm-wallet" element={<ConfirmWallet />} />
          <Route path="/identity" element={<Identity />} />
          <Route path="/applications" element={<Applications />} />
          <Route path="/applications/:id" element={<ApplicationDetails />} />
          <Route path="/publish-application" element={<PublishApplication />} />
          <Route path="/contexts" element={<Contexts />} />
          <Route path="/contexts/start-context" element={<StartContext />} />
          <Route path="/contexts/:id" element={<ContextDetails />} />
          <Route path="/export" element={<Export />} />
        </Routes>
      </BrowserRouter>
    </>
  );
}
