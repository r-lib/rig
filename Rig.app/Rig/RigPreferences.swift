//
//  GeneralPreferenceViewController.swift
//  Rig
//
//  Created by Gabor Csardi on 5/15/22.
//

import AppKit
import Preferences
import LaunchAtLogin

extension Preferences.PaneIdentifier {
    static let general = Self("general")
}

final class GeneralPreferenceViewController: NSViewController, PreferencePane {
    let preferencePaneIdentifier = Preferences.PaneIdentifier.general
    let preferencePaneTitle = "General"
    let toolbarItemIcon = NSImage(named: NSImage.Name("gear"))

    override var nibName: NSNib.Name? { "GeneralPreferenceViewController" }

    override func loadView() {
        let launchAtLoginButton = NSButton(checkboxWithTitle: "", target: nil, action: #selector(setLaunchAtLogin))
        launchAtLoginButton.state = LaunchAtLogin.isEnabled ? NSControl.StateValue.on : NSControl.StateValue.off

        let showRStudioButton = NSButton(checkboxWithTitle: "", target: nil, action: #selector(setShowRStudio))
        showRStudioButton.state = UserDefaults.standard.bool(forKey: "showRStudioMenu") ? NSControl.StateValue.on : NSControl.StateValue.off

        let showVersionNumberButton = NSButton(checkboxWithTitle: "", target: nil, action: #selector(setShowVersionNumber))
        showVersionNumberButton.state = UserDefaults.standard.bool(forKey: "versionShowNumber") ? NSControl.StateValue.on : NSControl.StateValue.off

        let grid = NSGridView(views: [
            [NSTextField(labelWithString: ""), NSTextField(labelWithString: ""), NSTextField(labelWithString: "    ")],
            [NSTextField(labelWithString: "    Launch at login"), launchAtLoginButton],
            [NSTextField(labelWithString: "    Show RStudio menu"), showRStudioButton],
            [NSTextField(labelWithString: "    Show version number"), showVersionNumberButton],
            [NSTextField(labelWithString: ""), NSTextField(labelWithString: ""), NSTextField(labelWithString: "    ")],
        ])
        grid.column(at: 0).xPlacement = NSGridCell.Placement.trailing
        grid.rowSpacing = 2
        self.view = grid
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        self.preferredContentSize = NSSize(width: 200, height: 110)
    }

    @objc func setLaunchAtLogin(_ sender: NSButton?) {
        LaunchAtLogin.isEnabled = sender!.state == NSControl.StateValue.on
    }

    @objc func setShowRStudio(_ sender: NSButton?) {
        UserDefaults.standard.set(sender!.state == NSControl.StateValue.on, forKey: "showRStudioMenu")
    }

    @objc func setShowVersionNumber(_ sender: NSButton?) {
        UserDefaults.standard.set(sender!.state == NSControl.StateValue.on, forKey: "versionShowNumber")
        (NSApp.delegate as? AppDelegate)?.setStatusBarTitle()
    }
}
