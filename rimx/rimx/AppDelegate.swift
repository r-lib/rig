//
//  AppDelegate.swift
//  rimx
//
//  Created by Gabor Csardi on 5/13/22.
//

import Foundation
import AppKit

class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    // keep status item and menu separate
    var statusBarItem: NSStatusItem!
    var statusBarMenu: NSMenu!

    func applicationDidFinishLaunching(_ aNotification: Notification) {
        let statusBar = NSStatusBar.system
        statusBarItem = statusBar.statusItem(withLength: NSStatusItem.variableLength)
        statusBarItem.button?.title = "R 4.2 (arm)"

        statusBarItem.button?.action = #selector(self.statusBarButtonClicked(sender:))
        statusBarItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])

        statusBarMenu = setupMenus()
    }

    @objc func setupMenus() -> NSMenu {
        let menu = NSMenu()
        menu.delegate = self

        var buffer = Data(count: 1024)
        var n = buffer.count
        buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
            rim_get_default(p, n)
            // TODO: error
        })
        var current = String(data: buffer.filter({ $0 != 0 }), encoding: .utf8)!

        print(current.count)

        buffer.withUnsafeMutableBytes({(p: UnsafeMutablePointer<CChar>) -> Void in
            rim_list(p, n)
            // TODO: error
        })

        var list = String(data: buffer.filter({ $0 != 0 }), encoding: .utf8)!

        print(list)
        print(list.count)

        let one = NSMenuItem(title: "R " + current, action: #selector(didTapOne) , keyEquivalent: "1")
        menu.addItem(one)

        let two = NSMenuItem(title: "R 4.2", action: #selector(didTapTwo) , keyEquivalent: "2")
        menu.addItem(two)

        let three = NSMenuItem(title: "R 4.2 (arm)", action: #selector(didTapThree) , keyEquivalent: "3")
        menu.addItem(three)

        menu.addItem(NSMenuItem.separator())

        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))

        return menu
    }

    @objc func statusBarButtonClicked(sender: NSStatusBarButton) {
        let event = NSApp.currentEvent!
        if event.type ==  NSEvent.EventType.leftMouseUp {
            statusBarItem.menu = setupMenus()
            statusBarItem.button?.performClick(nil)
        } else {
            // TODO: what should we do for right click
        }
    }

    @objc func menuDidClose(_ menu: NSMenu) {
        statusBarItem.menu = nil // remove menu so button works as before
    }

    @objc func didTapOne() {
        print("1")
    }

    @objc func didTapTwo() {
        print("2")
    }

    @objc func didTapThree() {
        print("3")
    }
}
