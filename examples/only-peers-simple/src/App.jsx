import React from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Index from "./pages/Index";
import Login from "./pages/Login";
import Near from "./pages/Near";
import Metamask from "./pages/Metamask";

function App() {
  return (
    <BrowserRouter basename="/">
      <Routes>
        <Route path="/" element={<Index />} />
        <Route path="/login" element={<Login />} />
        <Route path="/near" element={<Near />} />
        <Route path="/metamask" element={<Metamask />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
