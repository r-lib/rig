/* eslint-env node */

const path = require("path");
const CopyWebpackPlugin = require("copy-webpack-plugin");

module.exports = (env, { mode }) => {
  const isDev = mode === "development";

  return {
    target: "node",
    mode: mode || "none",
    entry: {
      extension: "./src/extension.ts",
    },
    output: {
      path: path.resolve(__dirname, "dist"),
      filename: "[name].js",
      chunkFormat: "commonjs",
      libraryTarget: "commonjs",
      devtoolModuleFilenameTemplate: "[resource-path]",
    },
    externalsType: "node-commonjs",
    externals: {
      vs: "vs",
      vscode: "commonjs vscode",
    },
    resolve: {
      roots: [__dirname],
      extensions: [".js", ".ts"],
    },
    optimization: {
      minimize: !isDev,
    },
    module: {
      rules: [
        {
          test: /\.(ts?)?$/iu,
          use: {
            loader: "swc-loader",
          },
          exclude: /node_modules/u,
        },
      ],
    },
    plugins: [
      new CopyWebpackPlugin({
        patterns: [
          { from: "static", to: "static" },
          {
            from: "package.json",
            to: "package.json",
          },
        ].filter(Boolean),
      }),
    ].filter(Boolean),
    devtool: isDev ? "inline-cheap-module-source-map" : false,
    infrastructureLogging: {
      level: "log", // enables logging required for problem matchers
    },
  };
};
