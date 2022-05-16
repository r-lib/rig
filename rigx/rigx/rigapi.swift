//
//  rigapi.swift
//  rigx
//
//  Created by Gabor Csardi on 5/13/22.
//

import Foundation

func rigDefault() -> String? {
    var buffer = Data(count: 1024)
    let n = buffer.count
    var err: Int32 = 0
    buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        err = rig_get_default(p, n)
    })
    if err != 0 {
        return nil
    }
    // TODO: error

    let def = String(data: buffer.prefix(while: { $0 != 0 }), encoding: .utf8)!

    return def
}

func rigSetDefault(version: String) {
    var buffer = version.data(using: .utf8)!
    buffer.append(0)
    buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        let err = rig_set_default(p)
        // TODO: error
    })
}

func rigList() -> Array<String> {
    var buffer = Data(count: 1024)
    let n = buffer.count
    buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        rig_list(p, n)
        // TODO: error
    })

    var result: Array<String> = []
    var i = 0
    while i < buffer.count && buffer[i] != 0 {
        if buffer[i] == 0 { break }
        let start = i
        while i < buffer.count && buffer[i] != 0 {
            i += 1;
        }
        let end = i
        if end > start {
            let v = String(data: buffer.subdata(in: start..<end), encoding: .utf8)
            result.append(v!)
        }
        i += 1;
    }

    return result
}

func rigStartRStudio(version: String?, project: String?) {
    var version2: String = version ?? ""
    var project2: String = project ?? ""

    var bversion = version2.data(using: .utf8)!
    bversion.append(0)
    var bproject = project2.data(using: .utf8)!
    bproject.append(0)

    bversion.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        bproject.withUnsafeMutableBytes({(q: UnsafeMutablePointer<CChar>) -> Void in
            let err = rig_start_rstudio(p, q)
            // TODO: error
        })
    })


}
