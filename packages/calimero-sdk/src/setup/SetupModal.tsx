import { useCallback, useState } from "react";
import apiClient from "../api";
import React from "react";
import Spinner from "../components/loader/Spinner";
import styled from "styled-components";

export interface SetupModalProps {
  successRoute: () => void;
  getNodeUrl: () => string | null;
  setNodeUrl: (url: string) => void;
}

const Container = styled.div`
  display: flex;
  height: 100vh;
  justify-content: center;
  background-color: #111111;
`;

const Content = styled.div`
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: center;
`;

const Box = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  background-color: #1c1c1c;
  padding: 2rem;
  gap: 1rem;
  border-radius: 0.5rem;
`;

const Grid = styled.div`
  display: grid;
  justify-items: center;
  align-items: center;
  gap: 2rem;
  padding: 0 3.5rem;
`;

const Title = styled.div`
  color: white;
  font-size: 2.5rem;
  font-weight: 600;
`;

const Input = styled.input`
  width: 400px;
  padding: 0.5rem;
  border-radius: 0.375rem;
`;

const Error = styled.div`
  color: #ef4444;
`;

const Button = styled.button`
  background-color: #6b7280;
  color: white;
  width: 100%;
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 0.5rem;
  height: 46px;
  cursor: pointer;
  font-size: 1rem;
  font-weight: 500;
  border-radius: 0.375rem;
  border: none;
  outline: none;
  padding-top: 0.5rem;
  padding-left: 0.5rem;
  padding-right: 0.5rem;

  &:disabled {
    cursor: not-allowed;
  }
`;

const SetupModal: React.FC<SetupModalProps> = (props: SetupModalProps) => {
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
    <Container>
      <Content>
        <Box>
          <Grid>
            <Title>App setup</Title>
            {loading ? (
              <Spinner />
            ) : (
              <>
                <div>
                  <Input
                    type="text"
                    placeholder="node url"
                    inputMode="url"
                    value={url?.toString() || ""}
                    onChange={(e: { target: { value: string } }) => {
                      handleChange(e.target.value);
                    }}
                  />
                  <Error>{error}</Error>
                </div>
                <Button
                  disabled={!url}
                  onClick={() => {
                    checkConnection();
                  }}
                >
                  <span>Set node URL</span>
                </Button>
              </>
            )}
          </Grid>
        </Box>
      </Content>
    </Container>
  );
};

export default SetupModal;
