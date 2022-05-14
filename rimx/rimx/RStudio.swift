//
//  RStudio.swift
//  rimx
//
//  Created by Gabor Csardi on 5/14/22.
//

import Foundation
import AppKit

func recentRStudioProjects() -> Array<String>? {
    let home = ProcessInfo.processInfo.environment["HOME"]
    if home == nil { return nil }
    let path = home! + "/" + ".local/share/rstudio/monitored/lists/project_mru";

    let fileManager = FileManager.default
    if !fileManager.isReadableFile(atPath: path) { return nil }

    let text = try? String(contentsOfFile: path)
    if text == nil { return nil }
    var lines : [String] = text!.components(separatedBy: "\n")

    for (i, l) in lines.enumerated() {
        if l.prefix(2) == "~/" {
            lines[i] = String(home! + l.dropFirst(1))
        }
    }

    return lines
}
