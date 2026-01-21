/**
 * Adapted from https://github.com/microsoft/TypeScript/issues/29729
 * Since `string | 'foo'` doesn't offer auto completion
 */
declare type StringLiteralUnion<T extends U, U = string> = T | (U & {});
export declare type DownloadVersion = StringLiteralUnion<'insiders' | 'stable'>;
export declare type DownloadPlatform = StringLiteralUnion<'darwin' | 'win32-archive' | 'win32-x64-archive' | 'linux-x64'>;
export interface DownloadOptions {
    readonly cachePath: string;
    readonly version: DownloadVersion;
    readonly platform: DownloadPlatform;
}
/**
 * Download and unzip a copy of VS Code.
 * @returns Promise of `vscodeExecutablePath`.
 */
export declare function download(options?: Partial<DownloadOptions>): Promise<string>;
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
export declare function downloadAndUnzipVSCode(version?: DownloadVersion, platform?: DownloadPlatform): Promise<string>;
export {};
