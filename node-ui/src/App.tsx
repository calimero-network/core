import React from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Bootstrap from "./pages/Bootstrap";
import Identity from "./pages/Identity";
import Applications from "./pages/Applications";
import Contexts from "./pages/Contexts";
import StartContext from "./pages/StartContext";
import ContextDetails from "./pages/ContextDetails";
import Export from "./pages/Export";
import ApplicationDetails from "./pages/ApplicationDetails";
import PublishApplication from "./pages/PublishApplication";
import AddRelease from "./pages/AddRelease";
import Near from "./pages/Near";
import Metamask from "./pages/Metamask";

import "bootstrap/dist/css/bootstrap.min.css";

export default function App() {
  return (
    <>
      <BrowserRouter basename="/admin">
        <Routes>
          <Route path="/" element={<Bootstrap />} />
          <Route path="/near" element={<Near />} />
          <Route path="/metamask" element={<Metamask />} />
          <Route path="/identity" element={<Identity />} />
          <Route path="/applications" element={<Applications />} />
          <Route path="/applications/:id" element={<ApplicationDetails />} />
          <Route path="/publish-application" element={<PublishApplication />} />
          <Route path="/applications/:id/add-release" element={<AddRelease />} />
          <Route path="/contexts" element={<Contexts />} />
          <Route path="/contexts/start-context" element={<StartContext />} />
          <Route path="/contexts/:id" element={<ContextDetails />} />
          <Route path="/export" element={<Export />} />
        </Routes>
      </BrowserRouter>
    </>
  );
}
