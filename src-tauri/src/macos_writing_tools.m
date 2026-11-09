// Public AppKit APIs only. A real TextKit 2 text view avoids WebKit's <pre>
// exclusions, DOM mutations and attributed-selection conversion entirely.
#import <AppKit/AppKit.h>

typedef void (*ArgusWritingToolsCompletion)(int, const char *, void *);

static NSPasteboard *ArgusWritingToolsPasteboard(void) {
#ifdef ARGUS_WRITING_TOOLS_TESTING
    return [NSPasteboard pasteboardWithName:[NSString stringWithFormat:@"argus-writing-tools-regression-%d", NSProcessInfo.processInfo.processIdentifier]];
#else
    return NSPasteboard.generalPasteboard;
#endif
}

@interface ArgusWritingToolsController : NSObject <NSWindowDelegate, NSTextViewDelegate>
@property(nonatomic, strong) NSWindow *parent;
@property(nonatomic, strong) NSPanel *panel;
@property(nonatomic, strong) NSTextView *textView;
@property(nonatomic, strong) NSButton *applyButton;
@property(nonatomic, strong) NSButton *resultCopyButton;
@property(nonatomic, copy) NSString *text;
@property(nonatomic) BOOL canReplace;
@property(nonatomic) BOOL english;
@property(nonatomic) BOOL launched;
@property(nonatomic) ArgusWritingToolsCompletion completion;
@property(nonatomic) void *context;
@end

@implementation ArgusWritingToolsController
- (NSString *)zh:(NSString *)zh en:(NSString *)en { return self.english ? en : zh; }
- (void)finish:(int)action {
    if (!self.completion) return;
    ArgusWritingToolsCompletion completion = self.completion;
    self.completion = NULL;
    [NSNotificationCenter.defaultCenter removeObserver:self];
    NSString *result = self.textView ? self.textView.string : self.text;
    completion(action, result.UTF8String, self.context);
}
- (BOOL)copyText:(NSString *)text {
    NSPasteboard *board = ArgusWritingToolsPasteboard();
    [board clearContents];
    return [board setString:text forType:NSPasteboardTypeString];
}
- (void)copySelection:(id)sender {
    [self finish:[self copyText:self.text] ? 2 : -1];
}
- (void)endSheet:(int)action {
    if (self.panel.sheetParent) [self.parent endSheet:self.panel returnCode:action];
    else [self finish:action];
}
- (void)cancel:(id)sender {
    if (@available(macOS 15.2, *)) [self.textView.writingToolsCoordinator stopWritingTools];
    [self endSheet:0];
}
- (void)copyResult:(id)sender {
    if (@available(macOS 15.0, *)) { if (self.textView.writingToolsActive) return; }
    if ([self copyText:self.textView.string]) [self endSheet:2];
    else NSBeep();
}
- (void)apply:(id)sender {
    if (!self.canReplace) return;
    if (@available(macOS 15.0, *)) { if (self.textView.writingToolsActive) return; }
    // The button explicitly promises copying, so a stale source never loses
    // the result when the frontend refuses to overwrite a changed document.
    if ([self copyText:self.textView.string]) [self endSheet:1];
    else NSBeep();
}
- (BOOL)windowShouldClose:(NSWindow *)sender { [self cancel:nil]; return NO; }
- (void)textViewWritingToolsWillBegin:(NSTextView *)textView API_AVAILABLE(macos(15.0)) {
    self.applyButton.enabled = NO;
    self.resultCopyButton.enabled = NO;
}
- (void)textViewWritingToolsDidEnd:(NSTextView *)textView API_AVAILABLE(macos(15.0)) {
    self.applyButton.enabled = YES;
    self.resultCopyButton.enabled = YES;
}
- (NSButton *)button:(NSString *)title action:(SEL)action {
    NSButton *button = [NSButton buttonWithTitle:title target:self action:action];
    button.translatesAutoresizingMaskIntoConstraints = NO;
    return button;
}
- (void)presentPanel:(BOOL)showTools API_AVAILABLE(macos(15.0)) {
    self.panel = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 680, 460)
        styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable
        backing:NSBackingStoreBuffered defer:NO];
    self.panel.title = [self zh:@"写作工具" en:@"Writing Tools"];
    self.panel.minSize = NSMakeSize(self.canReplace ? 600 : 480, 320);
    self.panel.delegate = self;
    NSView *content = self.panel.contentView;
    NSTextField *hint = [NSTextField wrappingLabelWithString:self.canReplace
        ? [self zh:@"核对结果后，可复制并替换原选区。" en:@"Review the result, then copy and replace the original selection."]
        : [self zh:@"处理结果可复制使用。" en:@"Copy the result when you are ready."]];
    hint.translatesAutoresizingMaskIntoConstraints = NO;
    hint.textColor = NSColor.secondaryLabelColor;
    [content addSubview:hint];

    NSScrollView *scroll = [[NSScrollView alloc] initWithFrame:NSZeroRect];
    scroll.translatesAutoresizingMaskIntoConstraints = NO;
    scroll.hasVerticalScroller = YES;
    scroll.borderType = NSBezelBorder;
    self.textView = [[NSTextView alloc] initUsingTextLayoutManager:YES];
    self.textView.richText = NO;
    self.textView.allowsUndo = YES;
    self.textView.font = [NSFont systemFontOfSize:14];
    self.textView.textContainerInset = NSMakeSize(12, 12);
    self.textView.verticallyResizable = YES;
    self.textView.horizontallyResizable = NO;
    self.textView.autoresizingMask = NSViewWidthSizable;
    self.textView.textContainer.widthTracksTextView = YES;
    self.textView.writingToolsBehavior = NSWritingToolsBehaviorComplete;
    self.textView.allowedWritingToolsResultOptions = NSWritingToolsResultPlainText;
    self.textView.string = self.text;
    self.textView.delegate = self;
    scroll.documentView = self.textView;
    [content addSubview:scroll];

    NSButton *cancel = [self button:[self zh:@"取消" en:@"Cancel"] action:@selector(cancel:)];
    cancel.keyEquivalent = @"\e";
    NSButton *tools = [self button:[self zh:@"写作工具…" en:@"Writing Tools…"] action:@selector(showTools:)];
    self.resultCopyButton = [self button:[self zh:@"复制结果" en:@"Copy Result"] action:@selector(copyResult:)];
    self.applyButton = [self button:[self zh:@"复制并替换" en:@"Copy & Replace"] action:@selector(apply:)];
    NSStackView *buttons = [NSStackView stackViewWithViews:self.canReplace
        ? @[tools, cancel, self.resultCopyButton, self.applyButton] : @[tools, cancel, self.resultCopyButton]];
    buttons.translatesAutoresizingMaskIntoConstraints = NO;
    buttons.spacing = 8;
    [content addSubview:buttons];
    // Return remains a newline in the editor; finishing always requires a click.
    [NSLayoutConstraint activateConstraints:@[
        [hint.topAnchor constraintEqualToAnchor:content.topAnchor constant:16],
        [hint.leadingAnchor constraintEqualToAnchor:content.leadingAnchor constant:16],
        [hint.trailingAnchor constraintEqualToAnchor:content.trailingAnchor constant:-16],
        [scroll.topAnchor constraintEqualToAnchor:hint.bottomAnchor constant:12],
        [scroll.leadingAnchor constraintEqualToAnchor:content.leadingAnchor constant:16],
        [scroll.trailingAnchor constraintEqualToAnchor:content.trailingAnchor constant:-16],
        [scroll.bottomAnchor constraintEqualToAnchor:buttons.topAnchor constant:-12],
        [buttons.trailingAnchor constraintEqualToAnchor:content.trailingAnchor constant:-16],
        [buttons.bottomAnchor constraintEqualToAnchor:content.bottomAnchor constant:-16],
    ]];
    // beginSheet retains the completion, which retains this controller until
    // dismissal; buttons and the panel delegate do not own their target.
    [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(cancel:)
        name:NSWindowWillCloseNotification object:self.parent];
    [self.parent beginSheet:self.panel completionHandler:^(NSModalResponse response) {
        [self.panel orderOut:nil];
        [self finish:(int)response];
    }];
    [self.panel makeFirstResponder:self.textView];
    [self.textView selectAll:nil];
    if (showTools) dispatch_async(dispatch_get_main_queue(), ^{ [self showTools:nil]; });
}
- (void)showTools:(id)sender {
    if (@available(macOS 15.2, *)) {
        if (!self.completion || !self.panel.sheetParent) return;
        [self.panel makeFirstResponder:self.textView];
        [self.textView showWritingTools:nil];
    }
}
- (void)openTools:(id)sender {
    if (@available(macOS 15.2, *)) {
        if (!NSWritingToolsCoordinator.isWritingToolsAvailable || self.parent.attachedSheet) return;
        self.launched = YES;
        // Leave the context-menu tracking loop before presenting the sheet.
        dispatch_async(dispatch_get_main_queue(), ^{ [self presentPanel:YES]; });
    }
}
- (NSMenu *)menu {
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@""];
    menu.autoenablesItems = NO;
    if (@available(macOS 15.2, *)) menu.automaticallyInsertsWritingToolsItems = NO;
    NSMenuItem *copy = [[NSMenuItem alloc] initWithTitle:[self zh:@"复制" en:@"Copy"] action:@selector(copySelection:) keyEquivalent:@""];
    copy.target = self;
    [menu addItem:copy];
    [menu addItem:NSMenuItem.separatorItem];
    NSMenuItem *tools = [[NSMenuItem alloc] initWithTitle:[self zh:@"写作工具…" en:@"Writing Tools…"] action:@selector(openTools:) keyEquivalent:@""];
    tools.target = self;
    tools.enabled = NO;
    if (@available(macOS 15.2, *)) tools.enabled = NSWritingToolsCoordinator.isWritingToolsAvailable && !self.parent.attachedSheet;
    if (!tools.enabled) tools.toolTip = [self zh:@"需要在支持的 Mac 上启用 Apple Intelligence。" en:@"Enable Apple Intelligence on a supported Mac."];
    [menu addItem:tools];
    return menu;
}
@end

void argus_writing_tools_menu(void *window, const char *text, bool canReplace, bool english,
    ArgusWritingToolsCompletion completion, void *context) {
    @autoreleasepool {
        ArgusWritingToolsController *controller = [ArgusWritingToolsController new];
        controller.parent = (__bridge NSWindow *)window;
        controller.text = [NSString stringWithUTF8String:text];
        controller.canReplace = canReplace;
        controller.english = english;
        controller.completion = completion;
        controller.context = context;
        if (!controller.parent || !controller.text) { [controller finish:-1]; return; }
        [[controller menu] popUpMenuPositioningItem:nil atLocation:NSEvent.mouseLocation inView:nil];
        if (!controller.launched) [controller finish:0];
    }
}
