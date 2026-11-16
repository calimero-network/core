import XCTest

/// XCUITest: launches the app in the simulator against an in-app Calimero 1.0
/// cluster (`-uitest-mock`: a fake node that runs the real sealed channel and
/// mock attestation) and drives the real UI: a development-cluster sign-in,
/// the home screen, the SDK list, and sign out; and a Cloud sign-in whose
/// mock quote the production policy refuses.
final class LoginFlowUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchArguments = ["-uitest-mock"]
    }

    /// Taps `button`, then waits for `expected` to appear, retrying the tap if
    /// it doesn't (SwiftUI can drop a tap before its gestures are attached).
    private func tap(
        _ button: XCUIElement, untilExists expected: XCUIElement, timeout: TimeInterval = 5, retries: Int = 3,
        message: String = "", file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertTrue(button.waitForExistence(timeout: timeout), "button not found. \(message)", file: file, line: line)
        for _ in 0...retries {
            if expected.exists { return }
            if button.exists { button.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap() }
            if expected.waitForExistence(timeout: timeout) { return }
        }
        XCTFail("expected element never appeared after tapping. \(message)", file: file, line: line)
    }

    func testDevelopmentClusterSignInHomeAndSignOut() throws {
        app.launch()
        XCTAssertTrue(app.staticTexts["loginTitle"].waitForExistence(timeout: 5))
        tap(app.buttons["manualNode"], untilExists: app.textFields["nodeURLField"], message: "manual node form")
        tap(
            app.buttons["nodeConnectButton"], untilExists: app.staticTexts["homeUser"],
            message: "should reach Home after signing in to the development cluster")
        XCTAssertTrue(app.descendants(matching: .any)["sessionNote"].exists)
        tap(
            app.buttons["logoutButton"], untilExists: app.staticTexts["loginTitle"],
            message: "should return to sign-in after sign out")
    }

    func testACloudNodeWithAMockQuoteIsRefused() throws {
        app.launch()
        XCTAssertTrue(app.staticTexts["loginTitle"].waitForExistence(timeout: 5))
        tap(
            app.buttons["cloudSignInButton"], untilExists: app.descendants(matching: .any)["loginError"],
            message: "a production policy refuses a mock quote, inline")
        XCTAssertFalse(app.staticTexts["homeUser"].exists)
    }
}
