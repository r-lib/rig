"use strict";
/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
Object.defineProperty(exports, "__esModule", { value: true });
exports.resolveCliPathFromVSCodeExecutablePath = exports.getLatestInsidersMetadata = exports.insidersDownloadDirMetadata = exports.insidersDownloadDirToExecutablePath = exports.downloadDirToExecutablePath = exports.urlToOptions = exports.getVSCodeDownloadUrl = exports.systemDefaultPlatform = void 0;
const path = require("path");
const url_1 = require("url");
const request = require("./request");
const createHttpsProxyAgent = require("https-proxy-agent");
const createHttpProxyAgent = require("http-proxy-agent");
const fs_1 = require("fs");
switch (process.platform) {
    case 'darwin':
        exports.systemDefaultPlatform = 'darwin';
        break;
    case 'win32':
        exports.systemDefaultPlatform = 'win32-archive';
        break;
    default:
        exports.systemDefaultPlatform = 'linux-x64';
}
function getVSCodeDownloadUrl(version, platform) {
    const downloadPlatform = platform || exports.systemDefaultPlatform;
    if (version === 'insiders') {
        return `https://update.code.visualstudio.com/latest/${downloadPlatform}/insider`;
    }
    return `https://update.code.visualstudio.com/${version}/${downloadPlatform}/stable`;
}
exports.getVSCodeDownloadUrl = getVSCodeDownloadUrl;
let PROXY_AGENT = undefined;
let HTTPS_PROXY_AGENT = undefined;
if (process.env.npm_config_proxy) {
    PROXY_AGENT = createHttpProxyAgent(process.env.npm_config_proxy);
    HTTPS_PROXY_AGENT = createHttpsProxyAgent(process.env.npm_config_proxy);
}
if (process.env.npm_config_https_proxy) {
    HTTPS_PROXY_AGENT = createHttpsProxyAgent(process.env.npm_config_https_proxy);
}
function urlToOptions(url) {
    const parsed = new url_1.URL(url);
    const options = {};
    if (PROXY_AGENT && parsed.protocol.startsWith('http:')) {
        options.agent = PROXY_AGENT;
    }
    if (HTTPS_PROXY_AGENT && parsed.protocol.startsWith('https:')) {
        options.agent = HTTPS_PROXY_AGENT;
    }
    return options;
}
exports.urlToOptions = urlToOptions;
function downloadDirToExecutablePath(dir) {
    if (process.platform === 'win32') {
        return path.resolve(dir, 'Code.exe');
    }
    else if (process.platform === 'darwin') {
        return path.resolve(dir, 'Visual Studio Code.app/Contents/MacOS/Electron');
    }
    else {
        return path.resolve(dir, 'VSCode-linux-x64/code');
    }
}
exports.downloadDirToExecutablePath = downloadDirToExecutablePath;
function insidersDownloadDirToExecutablePath(dir) {
    if (process.platform === 'win32') {
        return path.resolve(dir, 'Code - Insiders.exe');
    }
    else if (process.platform === 'darwin') {
        return path.resolve(dir, 'Visual Studio Code - Insiders.app/Contents/MacOS/Electron');
    }
    else {
        return path.resolve(dir, 'VSCode-linux-x64/code-insiders');
    }
}
exports.insidersDownloadDirToExecutablePath = insidersDownloadDirToExecutablePath;
function insidersDownloadDirMetadata(dir) {
    let productJsonPath;
    if (process.platform === 'win32') {
        productJsonPath = path.resolve(dir, 'resources/app/product.json');
    }
    else if (process.platform === 'darwin') {
        productJsonPath = path.resolve(dir, 'Visual Studio Code - Insiders.app/Contents/Resources/app/product.json');
    }
    else {
        productJsonPath = path.resolve(dir, 'VSCode-linux-x64/resources/app/product.json');
    }
    const productJson = JSON.parse(fs_1.readFileSync(productJsonPath, 'utf-8'));
    return {
        version: productJson.commit,
        date: productJson.date
    };
}
exports.insidersDownloadDirMetadata = insidersDownloadDirMetadata;
async function getLatestInsidersMetadata(platform) {
    const remoteUrl = `https://update.code.visualstudio.com/api/update/${platform}/insider/latest`;
    return await request.getJSON(remoteUrl);
}
exports.getLatestInsidersMetadata = getLatestInsidersMetadata;
/**
 * Resolve the VS Code cli path from executable path returned from `downloadAndUnzipVSCode`.
 * You can use this path to spawn processes for extension management. For example:
 *
 * ```ts
 * const cp = require('child_process');
 * const { downloadAndUnzipVSCode, resolveCliPathFromExecutablePath } = require('vscode-test')
 * const vscodeExecutablePath = await downloadAndUnzipVSCode('1.36.0');
 * const cliPath = resolveCliPathFromExecutablePath(vscodeExecutablePath);
 *
 * cp.spawnSync(cliPath, ['--install-extension', '<EXTENSION-ID-OR-PATH-TO-VSIX>'], {
 *   encoding: 'utf-8',
 *   stdio: 'inherit'
 * });
 * ```
 *
 * @param vscodeExecutablePath The `vscodeExecutablePath` from `downloadAndUnzipVSCode`.
 */
function resolveCliPathFromVSCodeExecutablePath(vscodeExecutablePath) {
    if (process.platform === 'win32') {
        if (vscodeExecutablePath.endsWith('Code - Insiders.exe')) {
            return path.resolve(vscodeExecutablePath, '../bin/code-insiders.cmd');
        }
        else {
            return path.resolve(vscodeExecutablePath, '../bin/code.cmd');
        }
    }
    else if (process.platform === 'darwin') {
        return path.resolve(vscodeExecutablePath, '../../../Contents/Resources/app/bin/code');
    }
    else {
        if (vscodeExecutablePath.endsWith('code-insiders')) {
            return path.resolve(vscodeExecutablePath, '../bin/code-insiders');
        }
        else {
            return path.resolve(vscodeExecutablePath, '../bin/code');
        }
    }
}
exports.resolveCliPathFromVSCodeExecutablePath = resolveCliPathFromVSCodeExecutablePath;
