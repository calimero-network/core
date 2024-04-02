import React from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Bootstrap from "./pages/Bootstrap";
import ConfirmWallet from "./pages/ConfirmWallet";
import Identity from "./pages/Identity";

export default function App() {
  return (
    <>
      <BrowserRouter basename="/admin">
        <Routes>
          <Route path="/" element={<Bootstrap />} />
          <Route path="/confirm-wallet" element={<ConfirmWallet />} />
          <Route path="/identity" element={<Identity />} />
          <Route path="/applications" element={<Identity />} />
          <Route path="/keys" element={<Identity />} />
        </Routes>
      </BrowserRouter>
    </>
  );
}
