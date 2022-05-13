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
        statusBarItem.button?.title = "R " + rimDefault()

        statusBarItem.button?.action = #selector(self.statusBarButtonClicked(sender:))
        statusBarItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])

        statusBarMenu = setupMenus()
    }

    @objc func setupMenus() -> NSMenu {
        let menu = NSMenu()
        menu.delegate = self

        let def = rimDefault()
        let list = rimList()

        for v in list {
            let label = v == def ? "R " + v + " (default)" : "R " + v
            let item = NSMenuItem(title: label, action: #selector(dummy), keyEquivalent: "")
            menu.addItem(item)
        }

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

    @objc func dummy() {
        print("selected")
    }
}
