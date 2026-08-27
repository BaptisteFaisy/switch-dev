import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("the iOS target contains a shared buildable Xcode scheme", async () => {
  const [project, scheme] = await Promise.all([
    read("ios/CodexTerminal.xcodeproj/project.pbxproj"),
    read("ios/CodexTerminal.xcodeproj/xcshareddata/xcschemes/CodexTerminal.xcscheme"),
  ]);

  assert.match(project, /productType = "com\.apple\.product-type\.application"/);
  assert.match(project, /IPHONEOS_DEPLOYMENT_TARGET = 15\.0/);
  assert.match(project, /TARGETED_DEVICE_FAMILY = "1,2"/);
  assert.match(project, /PrivacyInfo\.xcprivacy in Resources/);
  assert.match(scheme, /BlueprintName = "CodexTerminal"/);
});

test("the iOS native bridge matches the web platform contract", async () => {
  const [controller, platform] = await Promise.all([
    read("ios/CodexTerminal/WebViewController.swift"),
    read("src/platform.ts"),
  ]);

  for (const method of ["getBaseUrl", "getToken", "setConfig", "openSettings"]) {
    assert.match(controller, new RegExp(`${method}:`));
  }
  assert.match(controller, /Object\.defineProperty\(window, "CstIOS"/);
  assert.match(platform, /CstIOS\?: MobileBridge/);
  assert.match(platform, /nativeWindow\.CstIOS \?\? nativeWindow\.CstAndroid/);
});

test("iOS targets the VPS and migrates the former PC route once", async () => {
  const [controller, info] = await Promise.all([
    read("ios/CodexTerminal/WebViewController.swift"),
    read("ios/CodexTerminal/Info.plist"),
  ]);

  assert.match(info, /https:\/\/azure-duello\.tail3a8bdf\.ts\.net/);
  assert.match(controller, /fallbackBaseURL = "https:\/\/azure-duello\.tail3a8bdf\.ts\.net"/);
  assert.match(controller, /legacyPCBaseURL = "https:\/\/pc-fixe-cst\.tail3a8bdf\.ts\.net"/);
  assert.match(controller, /vpsRouteMigrationKey/);
});

test("iOS keeps the admin token in Keychain and limits insecure networking", async () => {
  const [keychain, info, privacy, controller] = await Promise.all([
    read("ios/CodexTerminal/KeychainTokenStore.swift"),
    read("ios/CodexTerminal/Info.plist"),
    read("ios/CodexTerminal/PrivacyInfo.xcprivacy"),
    read("ios/CodexTerminal/WebViewController.swift"),
  ]);

  assert.match(keychain, /kSecClassGenericPassword/);
  assert.match(keychain, /kSecAttrAccessibleWhenUnlockedThisDeviceOnly/);
  assert.match(info, /<key>NSAllowsLocalNetworking<\/key>\s*<true\/>/);
  assert.doesNotMatch(info, /NSAllowsArbitraryLoads/);
  assert.match(privacy, /NSPrivacyAccessedAPICategoryUserDefaults/);
  assert.match(privacy, /<string>CA92\.1<\/string>/);
  assert.match(privacy, /<key>NSPrivacyTracking<\/key>\s*<false\/>/);
  assert.match(controller, /scheme == "https" \|\| \(scheme == "http" && isLocalHost\(host\)\)/);
});
