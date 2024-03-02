import { useContext, useState } from "react";
import { WebviewContext } from "./WebviewContext";

export const ExampleViewA = () => {
  const { callApi } = useContext(WebviewContext);
  const [bMessage, setBMessage] = useState<string>("");

  return (
    <div>
      <div style={{ display: "flex" }}>
        <button
          onClick={() => {
            callApi("showExampleViewB");
          }}
        >
          Show Example View B
        </button>
      </div>
      <div style={{ display: "flex", marginTop: 10 }}>
        <input
          type="text"
          value={bMessage}
          onChange={(e) => setBMessage(e.target.value)}
        />
        <button
          onClick={() => {
            callApi("sendMessageToExampleB", bMessage);
            setBMessage("");
          }}
        >
          Send to Example View B
        </button>
      </div>
    </div>
  );
};
