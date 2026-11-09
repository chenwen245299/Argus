// Compile/run through test-writing-tools-native.mjs. No model or general clipboard.
#define ARGUS_WRITING_TOOLS_TESTING 1
#import "../src-tauri/src/macos_writing_tools.m"

static int callbacks, resultAction;
static NSString *resultText;
static void completed(int action, const char *text, void *context) {
    callbacks++;
    resultAction = action;
    resultText = [NSString stringWithUTF8String:text];
}
static void check(BOOL value, NSString *message) {
    if (!value) { fprintf(stderr, "%s\n", message.UTF8String); exit(1); }
}
static void spin(void) {
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:0.15];
    while (until.timeIntervalSinceNow > 0) [NSRunLoop.currentRunLoop runMode:NSDefaultRunLoopMode beforeDate:until];
}
static ArgusWritingToolsController *controller(NSWindow *parent, BOOL replace, BOOL english) {
    ArgusWritingToolsController *c = [ArgusWritingToolsController new];
    c.parent = parent; c.text = @"中文 **重点** 与 $x_i^2$。\n\nSecond paragraph.";
    c.canReplace = replace; c.english = english; c.completion = completed;
    callbacks = 0; resultAction = -99; resultText = nil;
    return c;
}
int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (@available(macOS 15.2, *)) {
            [NSApplication sharedApplication];
            [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
            NSWindow *parent = [[NSWindow alloc] initWithContentRect:NSMakeRect(-2000,-2000,800,600)
                styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
            [parent orderFront:nil];
            NSPasteboard *board = ArgusWritingToolsPasteboard();
            [board clearContents]; [board setString:@"untouched" forType:NSPasteboardTypeString];
            ArgusWritingToolsController *c = controller(parent, YES, NO);
            NSMenu *menu = c.menu;
            check(!menu.automaticallyInsertsWritingToolsItems, @"must not expose the failing WebKit path alongside the native handoff");
            check(menu.numberOfItems == 3, @"copy and Writing Tools menu");
            check([menu.itemArray[2].title isEqualToString:@"写作工具…"], @"Chinese menu label");
            check(menu.itemArray[2].enabled == NSWritingToolsCoordinator.isWritingToolsAvailable, @"real system availability check");
            [c presentPanel:NO]; spin();
            check(c.textView.textLayoutManager != nil, @"TextKit 2 is required");
            check(c.textView.writingToolsBehavior == NSWritingToolsBehaviorComplete, @"complete native experience");
            check(c.textView.allowedWritingToolsResultOptions == NSWritingToolsResultPlainText, @"Markdown remains plain text");
            check([c.textView.string isEqualToString:c.text], @"Unicode and TeX roundtrip into native text storage");
            check(c.textView.selectedRange.length == c.text.length, @"the captured text is selected for Writing Tools");
            check(c.textView.bounds.size.width > 400, @"native text editor fills the panel");
            if (argc > 1) {
                [c.panel.contentView layoutSubtreeIfNeeded];
                NSBitmapImageRep *image = [c.panel.contentView bitmapImageRepForCachingDisplayInRect:c.panel.contentView.bounds];
                [c.panel.contentView cacheDisplayInRect:c.panel.contentView.bounds toBitmapImageRep:image];
                [[image representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:[NSString stringWithUTF8String:argv[1]] atomically:YES];
            }
            [c cancel:nil]; spin();
            check(callbacks == 1 && resultAction == 0, @"cancel resolves once");
            check([[board stringForType:NSPasteboardTypeString] isEqualToString:@"untouched"], @"cancel preserves clipboard");

            c = controller(parent, YES, YES); [c presentPanel:NO]; spin();
            check([c.panel.title isEqualToString:@"Writing Tools"], @"English panel title");
            [c textViewWritingToolsWillBegin:c.textView];
            check(!c.applyButton.enabled && !c.resultCopyButton.enabled, @"cannot finish during in-progress system rewrite");
            [c textViewWritingToolsDidEnd:c.textView];
            c.textView.string = @"Reviewed result $x_i^2$.";
            [c apply:nil]; spin();
            check(callbacks == 1 && resultAction == 1 && [resultText isEqualToString:@"Reviewed result $x_i^2$."], @"replace returns exact reviewed result once");
            check([[board stringForType:NSPasteboardTypeString] isEqualToString:resultText], @"explicit Copy & Replace preserves result for stale-source recovery");

            c = controller(parent, NO, YES); [c presentPanel:NO]; spin();
            [c apply:nil]; check(callbacks == 0, @"AI answers cannot request replacement");
            c.textView.string = @"Copied answer"; [c copyResult:nil]; spin();
            check(callbacks == 1 && resultAction == 2, @"copy-only result");

            c = controller(parent, YES, NO); [c presentPanel:NO]; spin();
            [NSNotificationCenter.defaultCenter postNotificationName:NSWindowWillCloseNotification object:parent]; spin();
            check(callbacks == 1 && resultAction == 0, @"closing the parent cancels the pending sheet");
            [c finish:1]; check(callbacks == 1, @"late callbacks cannot resolve twice");
            [parent orderOut:nil];
            [board releaseGlobally];
            puts("Native TextKit 2 handoff, availability, layout, Unicode, copy/replace/cancel and window cleanup passed; Apple Intelligence inference was not run.");
        }
    }
    return 0;
}
