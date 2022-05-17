//
//  AppDelegate.swift
//  Rig
//
//  Created by Gabor Csardi on 5/13/22.
//

import Foundation
import AppKit
import Cocoa
import Preferences

class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {

    // keep status item and menu separate
    var statusBarItem: NSStatusItem!
    var statusBarMenu: NSMenu!
    var watcher: DirectoryWatcher?

    private var window: NSWindow!

    private lazy var preferencesWindowController = PreferencesWindowController(
        preferencePanes: [
            GeneralPreferenceViewController()
        ],
        style: .segmentedControl
    )

    func applicationDidFinishLaunching(_ aNotification: Notification) {
        let statusBar = NSStatusBar.system
        statusBarItem = statusBar.statusItem(withLength: NSStatusItem.variableLength)
        let def = rigDefault()
        if def == nil {
            statusBarItem.button?.title = "R"
        } else {
            statusBarItem.button?.title = "R " + def!
        }

        statusBarItem.button?.action = #selector(self.statusBarButtonClicked(sender:))
        statusBarItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])

        let fileManager = FileManager.default
        let versions = "/Library/Frameworks/R.framework/Versions"
        if fileManager.fileExists(atPath: versions) {
            watcher = DirectoryWatcher(withPath: versions, callback: { directoryWatcher in
                let def = rigDefault()
                if def != nil {
                    self.statusBarItem.button?.title = "R " + def!
                }
            })
        }
    }

    @objc func preferencesMenuItemActionHandler(_ sender: NSMenuItem) {
        preferencesWindowController.show()
    }

    @objc func setupMenus() -> NSMenu {
        let menu = NSMenu()
        menu.delegate = self

        let def = rigDefault()
        let list = rigList()

        // -- rstudio menu -----------------------------------------------------------------------------------------------------

        let rstudioMenu = NSMenu()
        rstudioMenu.addItem(NSMenuItem(title: "Default", action: #selector(startRStudio), keyEquivalent: ""))
        rstudioMenu.addItem(NSMenuItem.separator())
        for v in list {
            let label = "R " + v
            let item = NSMenuItem(title: label, action: #selector(startRStudio), keyEquivalent: "")
            rstudioMenu.addItem(item)
        }
        let rstudio = NSMenuItem(title: "RStudio", action: #selector(startRStudio), keyEquivalent: "")
        rstudio.submenu = rstudioMenu
        menu.addItem(NSMenuItem(title: "Start", action: nil, keyEquivalent: ""))
        menu.addItem(rstudio)

        // -- project menu -----------------------------------------------------------------------------------------------------

        let projects = recentRStudioProjects()
        if projects != nil {
            var cnt = 0
            let projectMenu = NSMenu()
            for p in projects! {
                if p == "" { continue }
                let fileName = String((p as NSString).lastPathComponent.split(separator: ".").first!)
                let submenu = NSMenu()
                let defitem = NSMenuItem(title: "Default", action: #selector(startRStudio2), keyEquivalent: "")
                defitem.representedObject = [p, "default"]
                submenu.addItem(defitem)
                submenu.addItem(NSMenuItem.separator())
                for v in list {
                    let label = "R " + v
                    let subitem = NSMenuItem(title: label, action: #selector(startRStudio2), keyEquivalent: "")
                    subitem.representedObject = [p, v]
                    submenu.addItem(subitem)
                }
                let item = NSMenuItem(title: fileName, action: #selector(startRStudio2), keyEquivalent: "")
                item.submenu = submenu
                item.representedObject = p
                cnt += 1;
                projectMenu.addItem(item)
            }
            let projects = NSMenuItem(title: "Recent Project", action: nil, keyEquivalent: "")
            projects.submenu = projectMenu
            menu.addItem(projects)
        }

        // -- version menu ------------------------------------------------------------------------------------------------------

        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Current R Version", action: nil, keyEquivalent: ""))
        for v in list {
            let label = "R " + v
            let item = NSMenuItem(title: label, action: #selector(selectVersion), keyEquivalent: "")
            if v == def {
                item.state = NSControl.StateValue.on
            }
            menu.addItem(item)
        }

        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Preferences...", action: #selector(preferencesMenuItemActionHandler), keyEquivalent: ""))
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

    @objc func selectVersion(_ sender: NSMenuItem?) {
        let ver = String(sender!.title.dropFirst(2))
        rigSetDefault(version: ver)
        // the directory watcher will update this, but nevertheless we update it as well
        let newver = rigDefault()
        if newver != nil {
            statusBarItem.button?.title = "R " + newver!
        }
    }

    @objc func startRStudio(_ sender: NSMenuItem?) {
        var ver = String(sender!.title.dropFirst(2))
        if ver == "fault" || ver == "tudio" { ver = rigDefault()! }
        rigStartRStudio(version: ver, project: nil)
    }

    @objc func startRStudio2(_ sender: NSMenuItem?) {
        var msg = sender!.representedObject! as! Array<String>
        var proj = msg[0] as! String
        var rver = msg[1] as! String
        if rver == "default" { rver = rigDefault()! }
        rigStartRStudio(version: rver, project: proj)
    }
}
