//
//  GeneralPreferenceViewController.swift
//  rigx
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
        let grid = NSGridView(views: [
            [NSTextField(labelWithString: ""), NSTextField(labelWithString: ""), NSTextField(labelWithString: "    ")],
            [NSTextField(labelWithString: "    Launch at login"), launchAtLoginButton],
            [NSTextField(labelWithString: ""), NSTextField(labelWithString: ""), NSTextField(labelWithString: "    ")],
        ])
        grid.column(at: 0).xPlacement = NSGridCell.Placement.trailing
        grid.rowSpacing = 2
        self.view = grid
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        self.preferredContentSize = NSSize(width: 200, height: 70)
    }

    @objc func setLaunchAtLogin(_ sender: NSButton?) {
        LaunchAtLogin.isEnabled = sender!.state == NSControl.StateValue.on
    }
}
