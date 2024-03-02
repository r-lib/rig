## Overview

This repo contains the boilerplate needed to quickly get your own VS Code Extension up and running using React Webviews.

- Easy to add new custom React-based Webviews
- Fast Refresh during local development
- Type-safe API for bidirectional postMessage communication

## Getting Started

First make sure you have git, Node v18 and VS Code installed, then run:

- `$ git clone git@github.com:sfc-gh-tkojima/vscode-react-webviews.git`
- `$ cd vscode-react-webviews`
- `$ npm install`
- [`$ code . `](https://code.visualstudio.com/docs/editor/command-line#_launching-from-command-line)
- Run and Debug (`F5`)

![Screenshot](static/screenshot.png)

Once launched you will see a new T-shirt icon in the left side bar. Click on it to activate the extension. The extension includes two sample Webviews to showcase the provided functionality:

- **ExampleViewA**
  - Provides a button to open the ExampleViewB window
  - Allows users to send messages from ExampleViewA to ExampleViewB
- **ExampleViewB**
  - Provides a Load File button that will call into the host process to read a file and render its text contents in the Webview
  - Listens for and displays messages sent from ExampleViewA

### Troubleshooting

- If the VS Code host window opens and then closes immediately or fails to open, view the output for why it failed.
  - In one case, not having all the recommended extensions (namely, the Typescript + Webpack Problem Matchers one) installed can cause the host window not to start up at all. See .vscode/extensions.json for the list of recommended extensions.

## Adding Views

Adding a new view requires you to update 3 files:

- package.json

  Add your view to the [`contributes.views`](https://code.visualstudio.com/api/references/contribution-points#contributes.views) section

- src/views/index.tsx

  Add your view to the Views const. They key should match the id you specified in `package.json`

  ```typescript
  export const Views = {
    exampleViewA: ExampleViewA,
    exampleViewB: ExampleViewB,
  } as const;
  ```

- src/extension.ts

  Call `registerAndConnectView("newViewId");` inside the activate function.

## postMessage API

To add a new postMessage API, update the `ViewApi` type in `src/viewApi.ts`. Then update the `api`object in`src/extension.ts` with a matching implementation.

To call the API from a Webview use the `callApi` function available on `WebviewContext`:

```typescript
import { useContext, useState } from "react";
import { WebviewContext } from "./WebviewContext";
...
export const ExampleViewA = () => {
 const {callApi} = useContext(WebviewContext);
 callApi("showInformationMessage", 'Hello there!');
};
```

## postMessage Events

Events are even easier to add than APIs. Update the `ViewEvents` type in `src/viewApi.ts`. You can then call `triggerEvent()` with your new event inside `extension.ts`.

To listen to thew new event from inside a Webview, use the `addListener` and `removeListener` functions on `WebviewContext`:

```typescript
import { useContext, useEffect, useState } from "react";
import { WebviewContext } from "./WebviewContext";

export const ExampleViewB = () => {
  const { callApi, addListener, removeListener } = useContext(WebviewContext);
  const [messages, setMessages] = useState<string[]>([]);

  useEffect(() => {
    const cb = (msg: string) => {
      setMessages([...messages, msg]);
    };
    addListener("exampleBMessage", cb);

    return () => {
      removeListener("exampleBMessage", cb);
    };
  }, [messages]);
};
```

You must use `useEffect` with `addListener` so that you can call `removeListener` whenever the component unmounts.

## Debugging

Sourcemaps are configured for both the extension host and view bundles. Setting breakpoints in VS Code editor windows will work out-of-the box!

## Building/distributing your Extension

To build your extension so you can distribute or publish it, run `npm run build`. This will output a `vsix` file into the `build` directory. You can then publish this to the VS Code Marketplace or install it manually.

For more info, consult the [VS Code docs](https://code.visualstudio.com/api/working-with-extensions/publishing-extension#packaging-extensions).
