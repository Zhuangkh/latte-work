// WKWebView-only tools. The bridge accepts single-use request nonces, never
// paths or commands from web pages. Request files are created by the main UI's
// Rust commands in a private, per-process directory and bound to one WebView.
#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>
#import <unistd.h>

@interface LatteBrowserTools : NSObject <WKScriptMessageHandler>
@end

static NSString *toolsRoot(void) {
    return [NSTemporaryDirectory() stringByAppendingPathComponent:
        [NSString stringWithFormat:@"latte-work-browser-tools-%d", getpid()]];
}
// Use Foundation's version query instead of Clang's availability runtime,
// which Rust's release linker does not implicitly link for Objective-C objects.
static BOOL macOSAtLeast(NSInteger major) {
    NSOperatingSystemVersion version = {major, 0, 0};
    return [NSProcessInfo.processInfo isOperatingSystemAtLeastVersion:version];
}
static void finishTool(NSString *directory, NSDictionary *result) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
    [data writeToFile:[directory stringByAppendingPathComponent:@"result.json"] atomically:YES];
}
@implementation LatteBrowserTools
- (void)userContentController:(WKUserContentController *)controller didReceiveScriptMessage:(WKScriptMessage *)message {
    (void)controller;
    if (![message.body isKindOfClass:NSString.class]) return;
    NSUUID *nonce = [[NSUUID alloc] initWithUUIDString:message.body];
    if (!nonce) return;
    NSString *directory = [toolsRoot() stringByAppendingPathComponent:nonce.UUIDString.lowercaseString];
    NSString *file = [directory stringByAppendingPathComponent:@"request.json"];
    NSDictionary *attrs = [NSFileManager.defaultManager attributesOfItemAtPath:file error:nil];
    if (!attrs || [attrs[NSFileSize] unsignedLongLongValue] > 8192) return;
    NSData *data = [NSData dataWithContentsOfFile:file];
    NSDictionary *request = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
    if (![request isKindOfClass:NSDictionary.class] ||
        ![request[@"view"] isEqualToString:[NSString stringWithFormat:@"%llu", (unsigned long long)(uintptr_t)(__bridge void *)message.webView]]) return;
    // Consume before any asynchronous operation. A web page cannot replay it.
    [NSFileManager.defaultManager removeItemAtPath:file error:nil];
    WKWebView *view = message.webView;
    NSString *operation = request[@"operation"];
    if ([operation isEqualToString:@"find"]) {
        NSString *query = request[@"query"];
        if (![query isKindOfClass:NSString.class] || query.length > 512) {
            finishTool(directory, @{@"error": @"查找内容过长"}); return;
        }
        if (macOSAtLeast(11)) {
            // Clang cannot infer availability from the Foundation query above.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
            WKFindConfiguration *config = [WKFindConfiguration new];
            config.backwards = [request[@"backwards"] boolValue];
            config.wraps = YES;
            config.caseSensitive = NO;
            [view findString:query withConfiguration:config completionHandler:^(WKFindResult *result) {
                finishTool(directory, @{@"found": @(result.matchFound)});
            }];
#pragma clang diagnostic pop
        } else finishTool(directory, @{@"error": @"页面查找需要 macOS 11 或更新版本"});
    } else if ([operation isEqualToString:@"clear"]) {
        [view.configuration.websiteDataStore removeDataOfTypes:WKWebsiteDataStore.allWebsiteDataTypes
            modifiedSince:[NSDate dateWithTimeIntervalSince1970:0] completionHandler:^{
                finishTool(directory, @{@"ok": @YES});
            }];
    } else if ([operation isEqualToString:@"capture"]) {
        if (view.bounds.size.width <= 0 || view.bounds.size.height <= 0 ||
            view.bounds.size.width > 4096 || view.bounds.size.height > 4096) {
            finishTool(directory, @{@"error": @"截图区域过大或不可见"}); return;
        }
        WKSnapshotConfiguration *config = [WKSnapshotConfiguration new];
        config.rect = view.bounds;
        config.snapshotWidth = @(view.bounds.size.width);
        [view takeSnapshotWithConfiguration:config completionHandler:^(NSImage *image, NSError *error) {
            if (error || !image) { finishTool(directory, @{@"error": @"网页截图失败"}); return; }
            CGImageRef cg = [image CGImageForProposedRect:NULL context:nil hints:nil];
            NSBitmapImageRep *bitmap = cg ? [[NSBitmapImageRep alloc] initWithCGImage:cg] : nil;
            NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
            if (!png || png.length > 64 * 1024 * 1024 ||
                ![png writeToFile:[directory stringByAppendingPathComponent:@"capture.png"] atomically:YES]) {
                finishTool(directory, @{@"error": @"无法保存网页截图"}); return;
            }
            finishTool(directory, @{@"ok": @YES});
        }];
    } else finishTool(directory, @{@"error": @"浏览器操作无效"});
}
@end

@interface WKWebView (LatteBrowserConfiguration)
- (instancetype)latte_initWithFrame:(NSRect)frame configuration:(WKWebViewConfiguration *)configuration;
@end
@implementation WKWebView (LatteBrowserConfiguration)
+ (void)load {
    method_exchangeImplementations(class_getInstanceMethod(self, @selector(initWithFrame:configuration:)),
        class_getInstanceMethod(self, @selector(latte_initWithFrame:configuration:)));
}
- (instancetype)latte_initWithFrame:(NSRect)frame configuration:(WKWebViewConfiguration *)configuration {
    // Only browser surfaces request incognito during construction. Keep the
    // workbench's default persistent data store completely separate.
    if (!configuration.websiteDataStore.persistent) {
        static WKWebsiteDataStore *store;
        static LatteBrowserTools *tools;
        static dispatch_once_t once;
        dispatch_once(&once, ^{
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
            if (macOSAtLeast(14)) store = [WKWebsiteDataStore dataStoreForIdentifier:
                [[NSUUID alloc] initWithUUIDString:@"9CFF5FA3-9459-4D90-94DB-EE907D324FB8"]];
#pragma clang diagnostic pop
            else store = WKWebsiteDataStore.nonPersistentDataStore;
            tools = [LatteBrowserTools new];
        });
        configuration.websiteDataStore = store;
        [configuration.userContentController addScriptMessageHandler:tools name:@"latteBrowserTools"];
    }
    return [self latte_initWithFrame:frame configuration:configuration];
}
@end
