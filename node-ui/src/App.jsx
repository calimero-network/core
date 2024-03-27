import React from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Bootstrap from "./pages/Bootstrap";
import ConfirmWallet from "./pages/ConfirmWallet";

export default function App() {
  return (
    <>
    <BrowserRouter basename="/admin">
      <Routes>
      <Route path="/" element={<Bootstrap />}/>
      <Route path="/confirm-wallet" element={<ConfirmWallet />}/>
          {/* <Route index element={<Home />} />
          <Route path="blogs" element={<Blogs />} />
          <Route path="contact" element={<Contact />} />
          <Route path="*" element={<NoPage />} /> */}
      </Routes>
    </BrowserRouter>
    </>
  );
}
