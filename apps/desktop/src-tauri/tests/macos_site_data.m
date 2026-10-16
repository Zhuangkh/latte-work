// Real, ephemeral WebKit cookie store; no window, browsing or user profile.
#import "../src/macos_site_data.m"
#import <assert.h>
@interface SiteDataTestView : NSObject
@property(nonatomic, strong) NSURL *URL;
@property(nonatomic, strong) WKWebViewConfiguration *configuration;
@end
@implementation SiteDataTestView
@end
static void waitFor(BOOL (^finished)(void)) {
    NSDate *start = [NSDate date];
    while (!finished() && -start.timeIntervalSinceNow < 8)
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    assert(finished());
}
static NSDictionary *request(SiteDataTestView *view, NSDictionary *query) {
    NSString *directory = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
    [NSFileManager.defaultManager createDirectoryAtPath:directory withIntermediateDirectories:NO attributes:nil error:nil];
    NSString *input = [[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:query options:0 error:nil] encoding:NSUTF8StringEncoding];
    latteBrowserSiteData((WKWebView *)view, directory, input);
    NSString *result = [directory stringByAppendingPathComponent:@"result.json"];
    waitFor(^BOOL{ return [NSFileManager.defaultManager fileExistsAtPath:result]; });
    NSDictionary *reply = [NSJSONSerialization JSONObjectWithData:[NSData dataWithContentsOfFile:result] options:0 error:nil];
    [NSFileManager.defaultManager removeItemAtPath:directory error:nil];
    return reply;
}
static NSArray<NSHTTPCookie *> *cookies(WKWebsiteDataStore *store) {
    __block NSArray *values = nil;
    [store.httpCookieStore getAllCookies:^(NSArray *items) { values = items; }];
    waitFor(^BOOL{ return values != nil; });
    return values;
}
int main(void) {
 @autoreleasepool {
    assert(siteDataMatchesHost(@"example.test", @"sub.example.test"));
    assert(siteDataMatchesHost(@"EXAMPLE.TEST", @"example.test"));
    assert(!siteDataMatchesHost(@"example.test", @"notexample.test"));
    assert(!siteDataMatchesHost(@"example.test", @"example.test.evil.test"));
    assert(!siteDataMatchesHost(@"", @"example.test"));
    assert(siteDataMatchesHost(@"[::1]", @"::1"));
    SiteDataTestView *view = [SiteDataTestView new];
    view.URL = [NSURL URLWithString:@"https://sub.example.test/path"];
    view.configuration = [WKWebViewConfiguration new];
    view.configuration.websiteDataStore = WKWebsiteDataStore.nonPersistentDataStore;
    WKWebsiteDataStore *store = view.configuration.websiteDataStore;
    for (NSString *domain in @[@"example.test", @"sub.example.test", @"other.test"]) {
        __block BOOL saved = NO;
        NSHTTPCookie *cookie = [NSHTTPCookie cookieWithProperties:@{NSHTTPCookieName:@"private-cookie", NSHTTPCookieValue:@"DO-NOT-EXPOSE", NSHTTPCookieDomain:domain, NSHTTPCookiePath:@"/"}];
        [store.httpCookieStore setCookie:cookie completionHandler:^{ saved = YES; }];
        waitFor(^BOOL{ return saved; });
    }
    NSDictionary *query = @{@"host":@"sub.example.test"};
    NSDictionary *data = request(view, query);
    assert(!data[@"error"] && [data[@"domains"] containsObject:@"example.test"]);
    assert([data[@"types"] containsObject:WKWebsiteDataTypeCookies]);
    assert(![[data description] containsString:@"DO-NOT-EXPOSE"]);
    NSString *token = data[@"token"];
    assert(request(view, @{@"host":@"sub.example.test", @"token":NSUUID.UUID.UUIDString})[@"error"]);
    assert(cookies(store).count == 3);
    assert(request(view, @{@"host":@"sub.example.test", @"token":token})[@"error"]);
    data = request(view, query);
    view.URL = [NSURL URLWithString:@"https://other.test/"];
    assert(request(view, @{@"host":@"sub.example.test", @"token":data[@"token"]})[@"error"]);
    assert(cookies(store).count == 3);
    view.URL = [NSURL URLWithString:@"https://sub.example.test/path"];
    data = request(view, query);
    view.configuration.websiteDataStore = WKWebsiteDataStore.nonPersistentDataStore;
    assert(request(view, @{@"host":@"sub.example.test", @"token":data[@"token"]})[@"error"]);
    assert(cookies(store).count == 3);
    view.configuration.websiteDataStore = store;
    data = request(view, query);
    NSMutableDictionary *expired = [objc_getAssociatedObject(view, &siteDataGrantKey) mutableCopy];
    expired[@"expires"] = @(NSProcessInfo.processInfo.systemUptime - 1);
    objc_setAssociatedObject(view, &siteDataGrantKey, expired, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    assert(request(view, @{@"host":@"sub.example.test", @"token":data[@"token"]})[@"error"]);
    assert(cookies(store).count == 3);
    data = request(view, query);
    NSDictionary *cleared = request(view, @{@"host":@"sub.example.test", @"token":data[@"token"]});
    assert([cleared[@"cleared"] boolValue]);
    NSArray *remaining = cookies(store);
    assert(remaining.count == 1 && [[remaining[0] domain] isEqual:@"other.test"]);
    assert(request(view, @{@"host":@"sub.example.test", @"token":data[@"token"]})[@"error"]);
    assert(cookies(store).count == 1);
    printf("PASS: native domain boundaries, shared subdomains, real cookie store, no cookie values, expiry, navigation, single-use clear and unrelated-site preservation\n");
 }
 return 0;
}
