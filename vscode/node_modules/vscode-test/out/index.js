"use strict";
/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
Object.defineProperty(exports, "__esModule", { value: true });
exports.resolveCliPathFromVSCodeExecutablePath = exports.runTests = exports.downloadAndUnzipVSCode = exports.download = void 0;
var download_1 = require("./download");
Object.defineProperty(exports, "download", { enumerable: true, get: function () { return download_1.download; } });
Object.defineProperty(exports, "downloadAndUnzipVSCode", { enumerable: true, get: function () { return download_1.downloadAndUnzipVSCode; } });
var runTest_1 = require("./runTest");
Object.defineProperty(exports, "runTests", { enumerable: true, get: function () { return runTest_1.runTests; } });
var util_1 = require("./util");
Object.defineProperty(exports, "resolveCliPathFromVSCodeExecutablePath", { enumerable: true, get: function () { return util_1.resolveCliPathFromVSCodeExecutablePath; } });
