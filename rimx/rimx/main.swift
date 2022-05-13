//
//  Main.swift
//  rimx
//
//  Created by Gabor Csardi on 5/13/22.
//

import AppKit

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate

_ = NSApplicationMain(CommandLine.argc, CommandLine.unsafeArgv)
