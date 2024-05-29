import React from "react";
import Footer from "@theme-original/DocItem/Footer";
import { FeedbackComponent } from "../../../components/FeedbackComponent";
import { HelpComponent } from "@site/src/components/HelpComponent";

export default function FooterWrapper(props) {
  return (
    <>
      <Footer {...props} />

      <div
        style={{ marginTop: "16px" }}
        className="theme-admonition theme-admonition-tip admonition_node_modules-@docusaurus-theme-classic-lib-theme-Admonition-Layout-styles-module alert alert--info"
      >
        <FeedbackComponent />
      </div>

      <div style={{ paddingTop: "16px" }}>
        <HelpComponent />
      </div>
    </>
  );
}
