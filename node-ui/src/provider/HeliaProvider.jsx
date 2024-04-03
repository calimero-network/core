/* eslint-disable no-console */

import { unixfs } from "@helia/unixfs";
import { createHelia } from "helia";
import PropTypes from "prop-types";
import React from "react";
import { useEffect, useState, useCallback, createContext } from "react";

export const HeliaContext = createContext({
  helia: null,
  fs: null,
  error: false,
  starting: true,
});

export const HeliaProvider = ({ children }) => {
  const [helia, setHelia] = useState(null);
  const [fs, setFs] = useState(null);
  const [starting, setStarting] = useState(true);
  const [error, setError] = useState(null);

  const startHelia = useCallback(async () => {
    if (helia) {
      console.info("helia already started");
    } else if (window.helia) {
      console.info("found a windowed instance of helia, populating ...");
      setHelia(window.helia);
      setFs(unixfs(helia));
      setStarting(false);
    } else {
      try {
        console.info("Starting Helia");
        const helia = await createHelia();
        setHelia(helia);
        setFs(unixfs(helia));
        setStarting(false);
      } catch (e) {
        console.error(e);
        setError(true);
      }
    }
  }, []);

  useEffect(() => {
    startHelia();
  }, []);

  return (
    <HeliaContext.Provider
      value={{
        helia,
        fs,
        error,
        starting,
      }}
    >
      {children}
    </HeliaContext.Provider>
  );
};

HeliaProvider.propTypes = {
  children: PropTypes.any,
};
