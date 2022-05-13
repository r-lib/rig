//
//  rimapi.swift
//  rimx
//
//  Created by Gabor Csardi on 5/13/22.
//

import Foundation

func rimDefault() -> String? {
    var buffer = Data(count: 1024)
    let n = buffer.count
    var err: Int32 = 0
    buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        err = rim_get_default(p, n)
    })
    if err != 0 {
        return nil
    }
    // TODO: error

    let def = String(data: buffer.prefix(while: { $0 != 0 }), encoding: .utf8)!

    return def
}

func rimSetDefault(version: String) {
    var buffer = version.data(using: .utf8)!
    buffer.append(0)
    buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        let err = rim_set_default(p)
        // TODO: error
    })
}

func rimList() -> Array<String> {
    var buffer = Data(count: 1024)
    let n = buffer.count
    buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
        rim_list(p, n)
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
