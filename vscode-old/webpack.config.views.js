/* eslint-env node */

const path = require("path");
const webpack = require("webpack");
const ReactRefreshWebpackPlugin = require("@pmmmwh/react-refresh-webpack-plugin");

module.exports = (env, { mode }) => {
  const isDev = mode === "development";

  return {
    target: "web",
    mode: mode || "none",
    entry: {
      views: "./src/views/index.tsx",
    },
    output: {
      path: path.resolve(__dirname, "dist"),
      filename: "[name].js",
      libraryTarget: "module",
    },
    experiments: {
      outputModule: true,
    },
    resolve: {
      roots: [__dirname],
      extensions: [".js", ".jsx", ".ts", ".tsx"],
    },
    optimization: {
      minimize: !isDev,
    },
    module: {
      rules: [
        {
          test: /\.(tsx?)?$/iu,
          use: {
            loader: "swc-loader",
          },
          exclude: /node_modules/u,
        },
      ],
    },
    devServer: {
      static: {
        directory: path.join(__dirname, "static"),
        publicPath: "/static",
      },
      allowedHosts: "all",
      headers: {
        "Access-Control-Allow-Origin": "*",
        "Access-Control-Allow-Methods":
          "GET, POST, PUT, DELETE, PATCH, OPTIONS",
        "Access-Control-Allow-Headers":
          "X-Requested-With, content-type, Authorization",
      },
      hot: true,
      client: {
        overlay: true,
      },
      compress: true,
      port: 18080,
    },
    plugins: [
      new webpack.ProvidePlugin({
        React: "react",
      }),
      isDev && new ReactRefreshWebpackPlugin(),
    ].filter(Boolean),
    devtool: isDev ? "inline-cheap-module-source-map" : false,
    infrastructureLogging: {
      level: "log", // enables logging required for problem matchers
    },
  };
};
