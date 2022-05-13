//
//  AppDelegate.swift
//  rimx
//
//  Created by Gabor Csardi on 5/13/22.
//

import AppKit

class AppDelegate: NSObject, NSApplicationDelegate {
    
    private var window: NSWindow!
    private var statusItem: NSStatusItem!
    
    func applicationDidFinishLaunching(_ aNotification: Notification) {
        
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = statusItem.button {
            button.title = "R 4.2 (arm)"
        }
        
        setupMenus()
    }
    
    func setupMenus() {
        let menu = NSMenu()
        
        let one = NSMenuItem(title: "R 4.1", action: #selector(didTapOne) , keyEquivalent: "1")
        menu.addItem(one)
        
        let two = NSMenuItem(title: "R 4.2", action: #selector(didTapTwo) , keyEquivalent: "2")
        menu.addItem(two)
        
        let three = NSMenuItem(title: "R 4.2 (arm)", action: #selector(didTapThree) , keyEquivalent: "3")
        menu.addItem(three)
        
        menu.addItem(NSMenuItem.separator())
        
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
        
        statusItem.menu = menu
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
