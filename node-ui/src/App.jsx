import React from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Bootstrap from "./pages/Bootstrap";
import ConfirmWallet from "./pages/ConfirmWallet";
import Identity from "./pages/Identity";
import Applications from "./pages/Applications";

import "bootstrap/dist/css/bootstrap.min.css";
import UploadApp from "./pages/UploadApp";
import Contexts from "./pages/Contexts";

export default function App() {
  return (
    <>
      <BrowserRouter basename="/admin">
        <Routes>
          <Route path="/" element={<Bootstrap />} />
          <Route path="/confirm-wallet" element={<ConfirmWallet />} />
          <Route path="/identity" element={<Identity />} />
          <Route path="/applications" element={<Applications />} />
          <Route path="/upload-app" element={<UploadApp />} />
          <Route path="/contexts" element={<Contexts />} />
        </Routes>
      </BrowserRouter>
    </>
  );
}
