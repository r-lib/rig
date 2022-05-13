//
//  AppDelegate.swift
//  rimx
//
//  Created by Gabor Csardi on 5/13/22.
//

import AppKit

class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {

    private var statusItem: NSStatusItem!
    private var menu: NSMenu!

    func applicationDidFinishLaunching(_ aNotification: Notification) {

        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.title = "R 4.2 (arm)"

        statusItem.button?.action = #selector(self.statusBarButtonClicked(sender:))
        statusItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
    }

    func setupMenus() -> NSMenu {
        let menu = NSMenu()

        let one = NSMenuItem(title: "R 4.1", action: #selector(didTapOne) , keyEquivalent: "1")
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
            statusItem.menu = setupMenus()
            statusItem.button?.performClick(nil)
        } else {
            // what should we do for right click?
        }
    }

    @objc func menuDidClose(_ menu: NSMenu) {
        statusItem.menu = nil // remove menu so button works as before
    }

    private func changeStatusBarButton(number: Int) {
        if let button = statusItem.button {
            button.image = NSImage(named: NSImage.Name("gear"))
        }
    }

    @objc func didTapOne() {
        changeStatusBarButton(number: 1)
    }

    @objc func didTapTwo() {
        changeStatusBarButton(number: 2)
    }

    @objc func didTapThree() {
        changeStatusBarButton(number: 3)
    }
}
