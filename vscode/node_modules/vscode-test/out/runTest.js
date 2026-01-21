"use strict";
/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
Object.defineProperty(exports, "__esModule", { value: true });
exports.runTests = void 0;
const cp = require("child_process");
const download_1 = require("./download");
/**
 * Run VS Code extension test
 *
 * @returns The exit code of the command to launch VS Code extension test
 */
async function runTests(options) {
    if (!options.vscodeExecutablePath) {
        options.vscodeExecutablePath = await download_1.downloadAndUnzipVSCode(options.version, options.platform);
    }
    let args = [
        // https://github.com/microsoft/vscode/issues/84238
        '--no-sandbox',
        '--extensionDevelopmentPath=' + options.extensionDevelopmentPath,
        '--extensionTestsPath=' + options.extensionTestsPath
    ];
    if (options.launchArgs) {
        args = options.launchArgs.concat(args);
    }
    return innerRunTests(options.vscodeExecutablePath, args, options.extensionTestsEnv);
}
exports.runTests = runTests;
async function innerRunTests(executable, args, testRunnerEnv) {
    return new Promise((resolve, reject) => {
        const fullEnv = Object.assign({}, process.env, testRunnerEnv);
        const cmd = cp.spawn(executable, args, { env: fullEnv });
        cmd.stdout.on('data', function (data) {
            console.log(data.toString());
        });
        cmd.stderr.on('data', function (data) {
            console.error(data.toString());
        });
        cmd.on('error', function (data) {
            console.log('Test error: ' + data.toString());
        });
        let finished = false;
        function onProcessClosed(code, signal) {
            if (finished) {
                return;
            }
            finished = true;
            console.log(`Exit code:   ${code !== null && code !== void 0 ? code : signal}`);
            if (code === null) {
                reject(signal);
            }
            else if (code !== 0) {
                reject('Failed');
            }
            console.log('Done\n');
            resolve(code !== null && code !== void 0 ? code : -1);
        }
        cmd.on('close', onProcessClosed);
        cmd.on('exit', onProcessClosed);
    });
}
