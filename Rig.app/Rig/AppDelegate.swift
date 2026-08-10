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
    // The directory `watcher` is watching, see updateWatcher().
    var watchedPath: String?

    private var window: NSWindow!

    private lazy var preferencesWindowController = PreferencesWindowController(
        preferencePanes: [
            GeneralPreferenceViewController()
        ],
        style: .segmentedControl
    )

    func setStatusBarTitle() {
        let def = try? rigDefault()
        if def == nil {
            statusBarItem.button?.title = "R"
        } else {
            let lib = try? rigLibDefault()
            let libstr = (lib == nil || lib! == "main") ? "" : " (" + lib!  + ")"
            statusBarItem.button?.title = "R " + def! + libstr
        }
    }

    func applicationDidFinishLaunching(_ aNotification: Notification) {
        let statusBar = NSStatusBar.system
        statusBarItem = statusBar.statusItem(withLength: NSStatusItem.variableLength)
        setStatusBarTitle()
        statusBarItem.button?.action = #selector(self.statusBarButtonClicked(sender:))
        statusBarItem.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])

        updateWatcher()
    }

    // The directory the R versions are installed into. This is
    // /Library/Frameworks/R.framework/Versions in admin mode and
    // ~/.local/share/rig/r in user mode, so ask rig instead of hard-coding it.
    func rRoot() -> String {
        return (try? rigRRoot()) ?? "/Library/Frameworks/R.framework/Versions"
    }

    // Watch the R root directory for changes, to keep the menu bar title in
    // sync with `rig default`. If the root does not exist yet -- in user mode
    // it is only created by the first `rig add` -- watch its closest existing
    // parent instead, and switch over to the root itself once it shows up.
    func updateWatcher() {
        guard let path = closestExistingDirectory(rRoot()) else { return }
        if path == watchedPath { return }
        watchedPath = path
        watcher = DirectoryWatcher(withPath: path, callback: { [weak self] _ in
            // Re-arming the watcher deallocates the one whose callback we are
            // in, so do this after the event handler returned.
            DispatchQueue.main.async {
                guard let self = self else { return }
                self.setStatusBarTitle()
                self.updateWatcher()
            }
        })
    }

    func closestExistingDirectory(_ path: String) -> String? {
        let fileManager = FileManager.default
        var url = URL(fileURLWithPath: path).standardizedFileURL
        while true {
            if fileManager.fileExists(atPath: url.path) {
                return url.path
            }
            let parent = url.deletingLastPathComponent().standardizedFileURL
            if parent.path == url.path {
                return nil
            }
            url = parent
        }
    }

    @objc func preferencesMenuItemActionHandler(_ sender: NSMenuItem) {
        preferencesWindowController.show()
    }

    @objc func setupMenus() -> NSMenu {
        let menu = NSMenu()
        menu.delegate = self

        let def = try? rigDefault() ?? ""
        let list: Array<InstalledVersion> = (try? rigList()) ?? []
        let libs: Array<String> = (try? rigLibList()) ?? []

        // -- rstudio menu -----------------------------------------------------------------------------------------------------

        let rstudioMenu = NSMenu()
        rstudioMenu.addItem(NSMenuItem(title: "Default", action: #selector(startRStudio), keyEquivalent: ""))
        rstudioMenu.addItem(NSMenuItem.separator())
        for v in list {
            let label = "R " + v.name
            let item = NSMenuItem(title: label, action: #selector(startRStudio), keyEquivalent: "")
            item.representedObject = v.name
            rstudioMenu.addItem(item)
        }
        let rstudio = NSMenuItem(title: "RStudio", action: #selector(startRStudio), keyEquivalent: "")
        rstudio.submenu = rstudioMenu
        menu.addItem(NSMenuItem(title: "Start", action: nil, keyEquivalent: ""))
        menu.addItem(rstudio)

        // -- project menu -----------------------------------------------------------------------------------------------------

        let projects = recentRStudioProjects()
        if projects != nil {
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
                    let label = "R " + v.name
                    let subitem = NSMenuItem(title: label, action: #selector(startRStudio2), keyEquivalent: "")
                    subitem.representedObject = [p, v.name]
                    submenu.addItem(subitem)
                }
                let item = NSMenuItem(title: fileName, action: #selector(startRStudio2), keyEquivalent: "")
                item.submenu = submenu
                item.representedObject = [p, "default"]
                projectMenu.addItem(item)
            }
            let projects = NSMenuItem(title: "Recent RStudio Project", action: nil, keyEquivalent: "")
            projects.submenu = projectMenu
            menu.addItem(projects)
        }

        // -- version menu ------------------------------------------------------------------------------------------------------

        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Current R Version", action: nil, keyEquivalent: ""))
        for v in list {
            let mark = NSAttributedString(
                string: v.version == "" ? " (broken?)" : (" (R " + v.version + ")"),
                attributes: [ NSAttributedString.Key.foregroundColor: NSColor.systemGray]
            )
            let label = NSMutableAttributedString(string: "R " + v.name + "  ")
            label.append(mark)
            let item = NSMenuItem()
            item.attributedTitle = label
            item.action = #selector(selectVersion)
            item.keyEquivalent = ""
            item.representedObject = v.name
            if v.name == def {
                item.state = NSControl.StateValue.on
            }
            menu.addItem(item)
        }

        // -- library version menu ----------------------------------------------------------------------------------------------

        if libs.count > 1 {
            menu.addItem(NSMenuItem.separator())
            menu.addItem(NSMenuItem(title: "Curent library", action: nil, keyEquivalent: ""))
            for lib in libs {
                let item = NSMenuItem()
                var label = lib
                if label.last != nil && label.last! == "*" {
                    label.removeLast()
                    item.state = NSControl.StateValue.on
                }
                item.title = label
                item.action = #selector(selectLibrary)
                item.keyEquivalent = ""
                menu.addItem(item)
            }
        }

        // ----------------------------------------------------------------------------------------------------------------------

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
        let ver = sender!.representedObject as! String
        let current = rRoot() + "/Current"
        let mode = (try? rigMode()) ?? "admin"
        let info = mode == "user" ? """
Check if you have write permissions to
\(current)
and its parent directory.
""" : """
Check if you have write permissions to
\(current)
and its parent directory. You can also run
rig system fix-permissions
from a shell. (It will need an admin password.)
"""
        do {
            try rigSetDefault(version: ver)
            // the directory watcher will update this, but nevertheless we update it as well
            self.setStatusBarTitle()
        } catch RigError.error(let msg) {
            setError(msg: "Failed to set default: \(msg)", info: info)
        } catch {
            setError(msg: "Failed to set default, unknown error", info: info)
        }
    }

    @objc func selectLibrary(_ sender: NSMenuItem?) {
        let lib = sender!.title
        let info = """
Check if `rig lib default` works form the command line.
Consider reporting a bug at https://github.com/r-lib/rig/issues.
Thank you!
"""
        do {
            try rigLibSetDefault(library: lib)
            self.setStatusBarTitle()
        } catch RigError.error(let msg) {
            setError(msg: "Failed to set default library: \(msg)", info: info)
        } catch {
            setError(msg: "Failed to set default library, unknown error", info: info)
        }
    }

    @objc func startRStudio2(_ sender: NSMenuItem?) {
        var msg = sender!.representedObject! as! Array<String>
        var proj = msg[0] as! String
        var rver: String? = msg[1] as! String
        if rver == "default" { rver = nil }
        startRStudio_(project: proj, version: rver)
    }

    @objc func startRStudio(_ sender: NSMenuItem?) {
        var ver: String? = String(sender!.title)
        if ver == "Default" || ver == "RStudio" {
            ver = nil
        } else {
          ver = sender!.representedObject as! String
        }

        startRStudio_(project: nil, version: ver)
    }

    func startRStudio_(project: String?, version: String?) {
        var info = """
Make sure that RStudio is installed and can start up.
You can try running
open -a RStudio
from a terminal.
"""
        do {
            try rigStartRStudio(version: version , project: project)
        } catch RigError.error(let msg) {
            if msg.contains("make-orthogonal") { info = "" }
            setError(msg: "Failed to start RStudio: \(msg)", info: info)
        } catch {
            setError(msg: "Failed to start RStudio, unknown error", info: info)
        }
    }

    func setError(msg: String, info: String) {
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = "Rig error"
        alert.informativeText = msg

        let txt = NSTextField(wrappingLabelWithString: info)
        let stackView = NSStackView(views: [txt])
        let width = txt.frame.width + stackView.spacing * 2
        let height = txt.frame.height + stackView.spacing
        stackView.setFrameSize(NSSize(width: width, height: height))
        stackView.translatesAutoresizingMaskIntoConstraints = true
        stackView.orientation = .vertical
        alert.accessoryView = stackView

        alert.runModal()
    }
}
