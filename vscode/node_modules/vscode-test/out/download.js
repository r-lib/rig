"use strict";
/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
Object.defineProperty(exports, "__esModule", { value: true });
exports.downloadAndUnzipVSCode = exports.download = void 0;
const fs = require("fs");
const path = require("path");
const cp = require("child_process");
const request = require("./request");
const del = require("./del");
const util_1 = require("./util");
const unzipper_1 = require("unzipper");
const stream_1 = require("stream");
const os_1 = require("os");
const util_2 = require("util");
const extensionRoot = process.cwd();
const vscodeStableReleasesAPI = `https://update.code.visualstudio.com/api/releases/stable`;
async function fetchLatestStableVersion() {
    const versions = await request.getJSON(vscodeStableReleasesAPI);
    if (!versions || !Array.isArray(versions) || !versions[0]) {
        throw Error('Failed to get latest VS Code version');
    }
    return versions[0];
}
async function isValidVersion(version) {
    const validVersions = await request.getJSON(vscodeStableReleasesAPI);
    return version === 'insiders' || validVersions.indexOf(version) !== -1;
}
/**
 * Download a copy of VS Code archive to `.vscode-test`.
 *
 * @param version The version of VS Code to download such as '1.32.0'. You can also use
 * `'stable'` for downloading latest stable release.
 * `'insiders'` for downloading latest Insiders.
 */
async function downloadVSCodeArchive(options) {
    if (!fs.existsSync(options.cachePath)) {
        fs.mkdirSync(options.cachePath);
    }
    const downloadUrl = util_1.getVSCodeDownloadUrl(options.version, options.platform);
    const text = `Downloading VS Code ${options.version} from ${downloadUrl}`;
    process.stdout.write(text);
    const res = await request.getStream(downloadUrl);
    if (res.statusCode !== 302) {
        throw 'Failed to get VS Code archive location';
    }
    const archiveUrl = res.headers.location;
    if (!archiveUrl) {
        throw 'Failed to get VS Code archive location';
    }
    const download = await request.getStream(archiveUrl);
    printProgress(text, download);
    return { stream: download, format: archiveUrl.endsWith('.zip') ? 'zip' : 'tgz' };
}
function printProgress(baseText, res) {
    if (!process.stdout.isTTY) {
        return;
    }
    const total = Number(res.headers['content-length']);
    let received = 0;
    let timeout;
    const reset = '\x1b[G\x1b[0K';
    res.on('data', chunk => {
        if (!timeout) {
            timeout = setTimeout(() => {
                process.stdout.write(`${reset}${baseText}: ${received}/${total} (${(received / total * 100).toFixed()}%)`);
                timeout = undefined;
            }, 100);
        }
        received += chunk.length;
    });
    res.on('end', () => {
        if (timeout) {
            clearTimeout(timeout);
        }
        console.log(`${reset}${baseText}: complete`);
    });
    res.on('error', err => {
        throw err;
    });
}
/**
 * Unzip a .zip or .tar.gz VS Code archive stream.
 */
async function unzipVSCode(extractDir, stream, format) {
    if (format === 'zip') {
        // note: this used to use Expand-Archive, but this caused a failure
        // on longer file paths on windows. Instead use unzipper, which does
        // not have this limitation.
        //
        // However it has problems that prevent it working on OSX:
        // - https://github.com/ZJONSSON/node-unzipper/issues/216 (avoidable)
        // - https://github.com/ZJONSSON/node-unzipper/issues/115 (not avoidable)
        if (process.platform !== 'darwin') {
            await new Promise((resolve, reject) => stream
                .pipe(unzipper_1.Extract({ path: extractDir }))
                .on('close', resolve)
                .on('error', reject));
        }
        else {
            const stagingFile = path.join(os_1.tmpdir(), `vscode-test-${Date.now()}.zip`);
            try {
                await util_2.promisify(stream_1.pipeline)(stream, fs.createWriteStream(stagingFile));
                await spawnDecompressorChild('unzip', ['-q', stagingFile, '-d', extractDir]);
            }
            finally {
                // fs.unlink(stagingFile, () => undefined);
            }
        }
    }
    else {
        // tar does not create extractDir by default
        if (!fs.existsSync(extractDir)) {
            fs.mkdirSync(extractDir);
        }
        await spawnDecompressorChild('tar', ['-xzf', '-', '-C', extractDir], stream);
    }
}
function spawnDecompressorChild(command, args, input) {
    const child = cp.spawn(command, args, { stdio: 'pipe' });
    input === null || input === void 0 ? void 0 : input.pipe(child.stdin);
    child.stderr.pipe(process.stderr);
    child.stdout.pipe(process.stdout);
    return new Promise((resolve, reject) => {
        child.on('error', reject);
        child.on('exit', code => code === 0 ? resolve() : reject(new Error(`Failed to unzip archive, exited with ${code}`)));
    });
}
/**
 * Download and unzip a copy of VS Code.
 * @returns Promise of `vscodeExecutablePath`.
 */
async function download(options) {
    var _a, _b;
    let version = options === null || options === void 0 ? void 0 : options.version;
    let platform = (_a = options === null || options === void 0 ? void 0 : options.platform) !== null && _a !== void 0 ? _a : util_1.systemDefaultPlatform;
    let cachePath = (_b = options === null || options === void 0 ? void 0 : options.cachePath) !== null && _b !== void 0 ? _b : path.resolve(extensionRoot, '.vscode-test');
    if (version) {
        if (version === 'stable') {
            version = await fetchLatestStableVersion();
        }
        else {
            /**
             * Only validate version against server when no local download that matches version exists
             */
            if (!fs.existsSync(path.resolve(cachePath, `vscode-${platform}-${version}`))) {
                if (!(await isValidVersion(version))) {
                    throw Error(`Invalid version ${version}`);
                }
            }
        }
    }
    else {
        version = await fetchLatestStableVersion();
    }
    const downloadedPath = path.resolve(cachePath, `vscode-${platform}-${version}`);
    if (fs.existsSync(downloadedPath)) {
        if (version === 'insiders') {
            const { version: currentHash, date: currentDate } = util_1.insidersDownloadDirMetadata(downloadedPath);
            const { version: latestHash, timestamp: latestTimestamp } = await util_1.getLatestInsidersMetadata(util_1.systemDefaultPlatform);
            if (currentHash === latestHash) {
                console.log(`Found insiders matching latest Insiders release. Skipping download.`);
                return Promise.resolve(util_1.insidersDownloadDirToExecutablePath(downloadedPath));
            }
            else {
                try {
                    console.log(`Remove outdated Insiders at ${downloadedPath} and re-downloading.`);
                    console.log(`Old: ${currentHash} | ${currentDate}`);
                    console.log(`New: ${latestHash} | ${new Date(latestTimestamp).toISOString()}`);
                    await del.rmdir(downloadedPath);
                    console.log(`Removed ${downloadedPath}`);
                }
                catch (err) {
                    console.error(err);
                    throw Error(`Failed to remove outdated Insiders at ${downloadedPath}.`);
                }
            }
        }
        else {
            console.log(`Found ${downloadedPath}. Skipping download.`);
            return Promise.resolve(util_1.downloadDirToExecutablePath(downloadedPath));
        }
    }
    try {
        const { stream, format } = await downloadVSCodeArchive({ version, platform, cachePath });
        await unzipVSCode(downloadedPath, stream, format);
        console.log(`Downloaded VS Code ${version} into ${downloadedPath}`);
    }
    catch (err) {
        console.error(err);
        throw Error(`Failed to download and unzip VS Code ${version}`);
    }
    if (version === 'insiders') {
        return Promise.resolve(util_1.insidersDownloadDirToExecutablePath(downloadedPath));
    }
    else {
        return util_1.downloadDirToExecutablePath(downloadedPath);
    }
}
exports.download = download;
/**
 * Download and unzip a copy of VS Code in `.vscode-test`. The paths are:
 * - `.vscode-test/vscode-<PLATFORM>-<VERSION>`. For example, `./vscode-test/vscode-win32-1.32.0`
 * - `.vscode-test/vscode-win32-insiders`.
 *
 * *If a local copy exists at `.vscode-test/vscode-<PLATFORM>-<VERSION>`, skip download.*
 *
 * @param version The version of VS Code to download such as `1.32.0`. You can also use
 * `'stable'` for downloading latest stable release.
 * `'insiders'` for downloading latest Insiders.
 * When unspecified, download latest stable version.
 *
 * @returns Promise of `vscodeExecutablePath`.
 */
async function downloadAndUnzipVSCode(version, platform = util_1.systemDefaultPlatform) {
    return await download({ version, platform });
}
exports.downloadAndUnzipVSCode = downloadAndUnzipVSCode;
