/* eslint-disable no-console */
import { useState, useCallback } from "react";
import { useHelia } from "./useHelia";

export const useUploadFile = () => {
  const { helia, fs, error, starting } = useHelia();
  const [cid, setCid] = useState(null);
  const [cidString, setCidString] = useState("");

  const commitWasm = useCallback(
    async (binary) => {
      if (!error && !starting) {
        try {
          const cid = await fs.addBytes(binary, helia.blockstore);
          setCid(cid);
          setCidString(cid.toString());
        } catch (e) {
          console.error(e);
        }
      }
    },
    [error, starting, helia, fs]
  );

  const fetchWasm = useCallback(async () => {
    if (!error && !starting && cid && helia && fs) {
      try {
        const fileChunks = [];
        for await (const chunk of fs.cat(cid)) {
          fileChunks.push(chunk);
        }

        const fileBlob = new Blob(fileChunks, { type: "application/wasm" });
        const fileObject = window.URL.createObjectURL(fileBlob);
        return fileObject;
      } catch (e) {
        console.error(e);
      }
    }
  }, [error, starting, cid, helia, fs]);

  return { cidString, commitWasm, fetchWasm };
};
