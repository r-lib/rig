import { vscode } from "./utilities/vscode";
import { VSCodeButton, VSCodeTextField } from "@vscode/webview-ui-toolkit/react";
import { useState, useCallback, useEffect } from 'react';
import "./App.css";

interface IRVersion {
  name: string;
  default: boolean;
  version: string;
  aliases: Array<string>;
  path: string;
};

function RVersionList() {
  const [versions, setVersions] = useState<Array<IRVersion>>();

  const listener = useCallback(event => {
    const message = event.data;
    switch (message.command) {
    case 'versions':
      setVersions(message.data);
      break;
    }
  }, []);

  useEffect(() => {
    window.addEventListener("message", listener);
    return () => {
        window.removeEventListener("message", listener);
    };
  }, [listener]);

  const listItems = versions?.map(v =>
    <li key={v.name}>
      <RVersion {...v} />
    </li>
  );

  return (
    <ul>{listItems}</ul>
  );
}

 function RVersion(version: IRVersion) {
   return <>
     {version.default ? "✅" : ""} {version.name} (R {version.version})
   </>
 }

interface IAppState {
  newversion: string;
}

function App() {
  const [value, setValue] = useState<string>('');

  function refreshClick() {
    vscode.postMessage({
      command: "refresh"
    });
  }

  function installClick() {
    console.log(value);
    vscode.postMessage({
      command: "install",
      version: value
    });
  }

  const onNew = (event: any) => {
    setValue(event.target.value)
  };

  return (
    <main>
      <h1>Installalled R versions</h1>
      <div>
        <RVersionList />
        <VSCodeButton onClick={refreshClick}>Refresh</VSCodeButton>
      </div>
      <h1>Install another R version</h1>
      <VSCodeTextField
       name="value"
       value={value}
       onInput={onNew}
       placeholder="release">
        Version
      </VSCodeTextField>
      <VSCodeButton onClick={installClick}>Install</VSCodeButton>
    </main>
  );
}

export default App;
