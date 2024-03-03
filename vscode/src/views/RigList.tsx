import { useContext, useState } from "react";
import { WebviewContext } from "./WebviewContext";

//import { execFile } from "node:child_process";
//import { promisify } from "node:util";
//const exec = promisify(execFile);

interface IRVersion {
  name: string;
  default: boolean;
  version: string;
  aliases: Array<string>;
  path: string;
  binary: string;
}

const RVersion = () => {
  const { callApi } = useContext(WebviewContext);
  const [data, setData] = useState<IRVersion>();

  return <div></div>;
};

const RVersionList = () => {
  const { callApi } = useContext(WebviewContext);
  const [data, setData] = useState<Array<IRVersion>>();

  return <div></div>;
};

async function rigRunLs() {
  //  const { stdout } = await exec("rig", ["ls", "--json"]);
  //  return stdout;
}

export const RigList = () => {
  const { callApi } = useContext(WebviewContext);
  const [messages, setMessages] = useState<string[]>([]);
  const [fileContent, setFileContent] = useState<string>();

  const getFileContents = async () => {
    setFileContent(await callApi("getFileContents"));
  };

  const versions = rigRunLs();
  //  console.log(JSON.parse(versions));
  return <RVersionList />;

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
