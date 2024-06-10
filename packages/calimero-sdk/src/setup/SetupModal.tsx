import { useCallback, useState } from "react";
import apiClient from "../api";
import React from "react";
import Spinner from "../components/loader/Spinner";

export interface SetupModalProps {
  successRoute: () => void;
  getNodeUrl: () => string | null;
  setNodeUrl: (url: string) => void;
}

export default function SetupModal(props: SetupModalProps) {
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [url, setUrl] = useState<string | null>(props.getNodeUrl());
  const MINIMUM_LOADING_TIME_MS = 1000;

  function validateUrl(value: string): boolean {
    try {
      new URL(value);
      return true;
    } catch (e) {
      return false;
    }
  }

  const handleChange = (url: string) => {
    setError("");
    setUrl(url);
  };

  const checkConnection = useCallback(async () => {
    if (!url) return;
    if (validateUrl(url.toString())) {
      setLoading(true);
      const timer = new Promise((resolve) =>
        setTimeout(resolve, MINIMUM_LOADING_TIME_MS)
      );

      const fetchData = apiClient.node().health({ url: url });
      Promise.all([timer, fetchData]).then(([, response]) => {
        if (response.data) {
          setError("");
          props.setNodeUrl(url);
          props.successRoute();
        } else {
          setError("Connection failed. Please check if node url is correct.");
        }
        setLoading(false);
      });
    } else {
      setError("Connection failed. Please check if node url is correct.");
    }
  }, [url]);

  return (
    <div className="flex h-screen justify-center bg-[#111111]">
      <div className="flex flex-col justify-center items-center">
        <div className="items-center bg-[#1C1C1C] p-8 gap-y-4 rounded-lg">
          <div className="grid justify-items-center items-center space-y-8 px-14">
            <div className="text-white text-4xl font-semibold">App setup</div>
            {loading ? (
              <Spinner />
            ) : (
              <>
                <div>
                  <input
                    type="text"
                    style={{ width: "400px" }}
                    className="p-2 rounded-md"
                    placeholder="node url"
                    inputMode="url"
                    value={url?.toString() || ""}
                    onChange={(e) => {
                      handleChange(e.target.value);
                    }}
                  />
                  <div className="text-red-500">{error}</div>
                </div>
                <button
                  style={{
                    backgroundColor: "#6b7280",
                    color: "white",
                    width: "100%",
                    display: "flex",
                    justifyContent: "center",
                    alignItems: "center",
                    gap: "0.5rem",
                    height: "46px",
                    cursor: "pointer",
                    fontSize: "1rem",
                    fontWeight: "500",
                    borderRadius: "0.375rem",
                    border: "none",
                    outline: "none",
                    paddingTop: "0.5rem",
                    paddingLeft: "0.5rem",
                    paddingRight: "0.5rem",
                  }}
                  disabled={!url}
                  onClick={() => {
                    checkConnection();
                  }}
                >
                  <span>Set node URL</span>
                </button>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
