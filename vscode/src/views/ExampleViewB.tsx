import { useContext, useEffect, useState } from "react";
import { WebviewContext } from "./WebviewContext";

export const ExampleViewB = () => {
  const { callApi, addListener, removeListener } = useContext(WebviewContext);
  const [messages, setMessages] = useState<string[]>([]);
  const [fileContent, setFileContent] = useState<string>();

  useEffect(() => {
    const cb = (msg: string) => {
      setMessages([...messages, msg]);
    };
    addListener("exampleBMessage", cb);

    return () => {
      removeListener("exampleBMessage", cb);
    };
  }, [messages]);

  const getFileContents = async () => {
    setFileContent(await callApi("getFileContents"));
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "row",
        alignItems: "flex-start",
      }}
    >
      <div
        style={{
          padding: 10,
        }}
      >
        <div
          style={{
            width: 500,
            height: 250,
            border: "1px solid grey",
            padding: 10,
            overflow: "scroll",
            marginBottom: 10,
          }}
        >
          <pre style={{ width: "100%", height: "100%" }}>{fileContent}</pre>
        </div>
        <button onClick={getFileContents}>Load File</button>
      </div>
      <div style={{ padding: 10 }}>
        <h3>Received Messages:</h3>
        <ul>
          {messages.map((msg, idx) => (
            <li key={idx}>{msg}</li>
          ))}
        </ul>
        <button onClick={() => setMessages([])}>Clear Messages</button>
      </div>
    </div>
  );
};
