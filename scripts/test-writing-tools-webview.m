// Real system WKWebView + real Vditor + production native panel, without model inference.
#define ARGUS_WRITING_TOOLS_TESTING 1
#import "../src-tauri/src/macos_writing_tools.m"
#import <WebKit/WebKit.h>

static WKWebView *webView;
static NSWindow *parent;
static BOOL finished;
static int phase;
static ArgusWritingToolsController *activeController;
static void fail(NSString *message) { fprintf(stderr,"%s\n",message.UTF8String);exit(1); }
static void evaluate(NSString *script) {
    [webView evaluateJavaScript:script completionHandler:^(id result,NSError *error) { if(error)fail(error.description); }];
}
static void completed(int action,const char *text,void *context) {
    NSString *value=[NSString stringWithUTF8String:text];
    NSDictionary *result=@{@"action": action==1?@"replace":action==2?@"copy":@"cancel",@"text":value};
    NSData *json=[NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
    evaluate([NSString stringWithFormat:@"window.__test.resolve(%@)",[[NSString alloc]initWithData:json encoding:NSUTF8StringEncoding]]);
}
@interface Harness : NSObject <WKScriptMessageHandler>
@end
@implementation Harness
- (void)userContentController:(WKUserContentController *)controller didReceiveScriptMessage:(WKScriptMessage *)message {
    NSDictionary *body=message.body;
    if([body[@"name"] isEqualToString:@"done"]){
        if(![body[@"ok"] boolValue])fail([NSString stringWithFormat:@"WKWebView phase %d failed: %@",phase,body]);
        if(phase==0){
            phase=1;
            evaluate(@"(()=>{const t=window.__test;const s=t.select('.answer-body');const before=document.querySelector('.answer-body').innerHTML;t.run(s).then(ok=>webkit.messageHandlers.writingTools.postMessage({name:'done',ok:ok&&document.querySelector('.answer-body').innerHTML===before}))})()");
        }else finished=YES;
        return;
    }
    if(![body[@"name"] isEqualToString:@"system_writing_tools"])fail(@"Unexpected native command");
    NSDictionary *args=body[@"args"];
    NSString *text=args[@"text"];
    if(phase==0 && ![text isEqualToString:@"第一段原始文字，需要润色。\n\n保留 **重点** 和 $x_i^2$。"])fail([@"Unexpected note selection: " stringByAppendingString:text]);
    if(phase==1 && ([args[@"canReplace"] boolValue] || ![text containsString:@"$x_i^2$"]))fail(@"Answer is copy-only and must retain formula source");
    activeController=[ArgusWritingToolsController new];
    activeController.parent=parent;activeController.text=text;
    activeController.canReplace=[args[@"canReplace"] boolValue];activeController.completion=completed;
    if(@available(macOS 15.0,*)) [activeController presentPanel:NO];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,(int64_t)(0.15*NSEC_PER_SEC)),dispatch_get_main_queue(),^{
        // Substitute only the unavailable model, retaining native focus changes,
        // text storage, action callback and the real frontend replacement path.
        if(phase==0){activeController.textView.string=@"原生面板处理后的笔记。\n\n保留 **重点** 和 $x_i^2$。";[activeController apply:nil];}
        else [activeController copyResult:nil];
    });
}
@end
int main(int argc,const char *argv[]) {
    @autoreleasepool {
        if(argc!=2)return 2;
        [NSApplication sharedApplication];[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        parent=[[NSWindow alloc]initWithContentRect:NSMakeRect(-2000,-2000,1000,800) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        Harness *harness=[Harness new];WKWebViewConfiguration *config=[WKWebViewConfiguration new];
        [config.userContentController addScriptMessageHandler:harness name:@"writingTools"];
        webView=[[WKWebView alloc]initWithFrame:NSMakeRect(0,0,1000,800) configuration:config];
        parent.contentView=webView;[parent orderFront:nil];
        [webView loadRequest:[NSURLRequest requestWithURL:[NSURL URLWithString:[NSString stringWithUTF8String:argv[1]]]]];
        NSDate *deadline=[NSDate dateWithTimeIntervalSinceNow:25];
        __block BOOL started=NO;
        while(!finished && deadline.timeIntervalSinceNow>0){
            [NSRunLoop.currentRunLoop runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
            if(!started)[webView evaluateJavaScript:@"!!window.__test?.editor.value?.getContent()?.includes('第一段原始文字')" completionHandler:^(id ready,NSError *error){
                if(started || ![ready boolValue])return;started=YES;
                evaluate(@"(()=>{const t=window.__test;t.invoke=(name,args)=>{webkit.messageHandlers.writingTools.postMessage({name,args});return new Promise(resolve=>t.resolve=resolve)};const root=document.querySelector('.vditor-ir .vditor-reset');const p=root.querySelector('p');const r=document.createRange();r.setStartBefore(p);r.setEndAfter(root.lastElementChild);const sel=getSelection();sel.removeAllRanges();sel.addRange(r);const s=t.capture(root);t.run(s).then(ok=>webkit.messageHandlers.writingTools.postMessage({name:'done',ok:ok&&t.editor.value.getContent().includes('原生面板处理后的笔记。')&&t.editor.value.getContent().includes('$x_i^2$'),content:t.editor.value.getContent()}))})()");
            }];
        }
        if(!finished)fail(started?@"Native WKWebView roundtrip timed out":@"WKWebView did not load the real editor fixture");
        [parent orderOut:nil];
        [ArgusWritingToolsPasteboard() releaseGlobally];
        puts("Real WKWebView note selection → native TextKit 2 sheet → Vditor replacement passed; AI answer handoff remained copy-only and retained TeX.");
    }
    return 0;
}
