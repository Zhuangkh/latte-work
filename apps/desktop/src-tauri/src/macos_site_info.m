#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <Security/Security.h>

static BOOL iconURL(NSURL *url) {
    return url.absoluteString.length <= 8192 &&
        ([@"https" isEqual:url.scheme] || [@"http" isEqual:url.scheme]) &&
        url.host.length && !url.user.length && !url.password.length &&
        ![@[@"tauri.localhost", @"ipc.localhost"] containsObject:url.host.lowercaseString];
}

// A small, cookie-free image request. Never buffer an unbounded response or
// follow a redirect into a custom protocol. Failure simply leaves the link icon.
@interface LatteSiteIcon : NSObject <NSURLSessionDataDelegate>
@property(nonatomic, strong) NSURLSession *session;
@property(nonatomic, strong) NSMutableData *data;
@property(nonatomic, strong) NSString *mime;
@property(nonatomic) NSUInteger redirects;
@property(nonatomic) BOOL failed;
@property(nonatomic, copy) void (^completion)(NSString *);
@end
@implementation LatteSiteIcon
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task
    didReceiveChallenge:(NSURLAuthenticationChallenge *)challenge
    completionHandler:(void (^)(NSURLSessionAuthChallengeDisposition, NSURLCredential *))finish {
    finish([challenge.protectionSpace.authenticationMethod isEqual:NSURLAuthenticationMethodServerTrust] ?
        NSURLSessionAuthChallengePerformDefaultHandling : NSURLSessionAuthChallengeCancelAuthenticationChallenge, nil);
}
- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)task
    didReceiveResponse:(NSURLResponse *)response completionHandler:(void (^)(NSURLSessionResponseDisposition))finish {
    NSSet *types = [NSSet setWithArray:@[@"image/png", @"image/jpeg", @"image/gif", @"image/webp", @"image/x-icon", @"image/vnd.microsoft.icon", @"image/svg+xml"]];
    BOOL accepted = [response isKindOfClass:NSHTTPURLResponse.class] &&
        [(NSHTTPURLResponse *)response statusCode] == 200 && iconURL(response.URL) &&
        [types containsObject:response.MIMEType.lowercaseString] && response.expectedContentLength <= 65536;
    self.mime = response.MIMEType.lowercaseString;
    if (!accepted) self.failed = YES;
    finish(accepted ? NSURLSessionResponseAllow : NSURLSessionResponseCancel);
}
- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)task didReceiveData:(NSData *)data {
    if (self.data.length + data.length > 65536) { self.failed = YES; [task cancel]; }
    else [self.data appendData:data];
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task
    willPerformHTTPRedirection:(NSHTTPURLResponse *)response newRequest:(NSURLRequest *)request
    completionHandler:(void (^)(NSURLRequest *))finish {
    BOOL accepted = ++self.redirects <= 3 && iconURL(request.URL) &&
        !([task.originalRequest.URL.scheme isEqual:@"https"] && ![request.URL.scheme isEqual:@"https"]);
    if (!accepted) self.failed = YES;
    finish(accepted ? request : nil);
}
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task didCompleteWithError:(NSError *)error {
    NSString *image = !self.failed && !error && self.data.length ?
        [NSString stringWithFormat:@"data:%@;base64,%@", self.mime, [self.data base64EncodedStringWithOptions:0]] : nil;
    self.completion(image);
    [session finishTasksAndInvalidate];
    self.session = nil;
}
@end

static void loadIcon(NSURL *url, void (^completion)(NSString *)) {
    LatteSiteIcon *loader = [LatteSiteIcon new];
    loader.data = [NSMutableData new];
    loader.completion = completion;
    NSURLSessionConfiguration *config = NSURLSessionConfiguration.ephemeralSessionConfiguration;
    config.HTTPShouldSetCookies = NO;
    config.HTTPCookieStorage = nil;
    config.URLCredentialStorage = nil;
    config.timeoutIntervalForRequest = 2;
    config.timeoutIntervalForResource = 3;
    loader.session = [NSURLSession sessionWithConfiguration:config delegate:loader delegateQueue:nil];
    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:url];
    [request setValue:@"image/*" forHTTPHeaderField:@"Accept"];
    [[loader.session dataTaskWithRequest:request] resume];
}

static NSDictionary *siteCertificate(SecTrustRef trust) {
    NSOperatingSystemVersion minimum = {12, 0, 0};
    if (!trust || ![NSProcessInfo.processInfo isOperatingSystemAtLeastVersion:minimum]) return nil;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"
    NSArray *chain = CFBridgingRelease(SecTrustCopyCertificateChain(trust));
#pragma clang diagnostic pop
    if (!chain.count) return nil;
    NSMutableDictionary *info = [NSMutableDictionary new];
    NSString *subject = CFBridgingRelease(SecCertificateCopySubjectSummary((__bridge SecCertificateRef)chain[0]));
    if (subject.length && subject.length <= 512) info[@"subject"] = subject;
    if (chain.count > 1) {
        NSString *issuer = CFBridgingRelease(SecCertificateCopySubjectSummary((__bridge SecCertificateRef)chain[1]));
        if (issuer.length && issuer.length <= 512) info[@"issuer"] = issuer;
    }
    return info.count ? info : nil;
}

void latteBrowserSiteInfo(WKWebView *view, NSString *directory, BOOL includeIcon) {
    static dispatch_semaphore_t slots;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ slots = dispatch_semaphore_create(4); });
    BOOL admitted = includeIcon && dispatch_semaphore_wait(slots, DISPATCH_TIME_NOW) == 0;
    NSString *url = view.URL.absoluteString ?: @"";
    NSString *title = view.title ?: @"";
    if (title.length > 256) title = [title substringToIndex:256];
    NSString *connection = @"unknown";
    if (!view.loading) {
        if ([view.URL.scheme isEqual:@"http"]) connection = @"insecure";
        else if ([view.URL.scheme isEqual:@"https"]) {
            SecTrustResultType result = kSecTrustResultInvalid;
            SecTrustRef trust = view.serverTrust;
            if (trust && SecTrustGetTrustResult(trust, &result) == errSecSuccess &&
                (result == kSecTrustResultProceed || result == kSecTrustResultUnspecified)) {
                connection = view.hasOnlySecureContent ? @"secure" : @"mixed";
            }
        }
    }
    NSMutableDictionary *info = [@{@"url":url, @"title":title, @"connection":connection} mutableCopy];
    NSDictionary *certificate = !view.loading && [view.URL.scheme isEqual:@"https"] ? siteCertificate(view.serverTrust) : nil;
    if (certificate) info[@"certificate"] = certificate;
    __block BOOL completed = NO;
    void (^finish)(NSString *) = ^(NSString *image) {
      dispatch_async(dispatch_get_main_queue(), ^{
        if (completed) return;
        completed = YES;
        if (admitted) dispatch_semaphore_signal(slots);
        NSMutableDictionary *snapshot = [info mutableCopy];
        if (image) snapshot[@"icon"] = image;
        NSData *data = [NSJSONSerialization dataWithJSONObject:snapshot options:0 error:nil];
        // A timed-out/closed Rust request removes the directory; never recreate it.
        [data writeToFile:[directory stringByAppendingPathComponent:@"result.json"] atomically:YES];
      });
    };
    if (!admitted || !iconURL(view.URL)) { finish(nil); return; }
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 8 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{ finish(nil); });
    // Only the document's declared icon URL is read; no page script or CSS is
    // retained. A fallback request stays on the already selected site origin.
    NSString *script = @"Array.from(document.querySelectorAll('link[rel~=icon]')).slice(0,8).map(e=>e.href).filter(u=>u.length<=8192)";
    [view evaluateJavaScript:script completionHandler:^(id value, NSError *error) {
        if (completed) return;
        NSURL *fallback = [NSURL URLWithString:@"/favicon.ico" relativeToURL:[NSURL URLWithString:url]].absoluteURL;
        NSURL *chosen = nil;
        if (!error && [value isKindOfClass:NSArray.class]) for (id item in value) {
            if (![item isKindOfClass:NSString.class]) continue;
            NSURL *candidate = [NSURL URLWithString:item];
            if (iconURL(candidate) && !([fallback.scheme isEqual:@"https"] && ![candidate.scheme isEqual:@"https"])) { chosen = candidate; break; }
        }
        NSURL *first = chosen ?: fallback;
        loadIcon(first, ^(NSString *image) {
            if (image || [first isEqual:fallback]) finish(image);
            else loadIcon(fallback, finish);
        });
    }];
}
