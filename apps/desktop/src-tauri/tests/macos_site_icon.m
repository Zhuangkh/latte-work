// Standalone native transport tests; no windows, real websites or login state.
#import "../src/macos_site_info.m"
#import <assert.h>

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        assert(argc == 2);
        for (NSString *value in @[@"file:///etc/hosts", @"javascript:alert(1)", @"https://user:password@example.com/icon", @"http://ipc.localhost/favicon.ico", @"http://tauri.localhost/favicon.ico"]) {
            assert(!iconURL([NSURL URLWithString:value]));
        }
        NSString *origin = [NSString stringWithUTF8String:argv[1]];
        NSDictionary *cases = @{@"/icon":@YES, @"/large":@NO, @"/stream-large":@NO, @"/html":@NO,
                                @"/redirect/3":@YES, @"/redirect/4":@NO, @"/auth":@NO, @"/slow":@NO};
        for (NSString *path in cases) {
            __block BOOL done = NO;
            __block NSString *image = nil;
            NSDate *start = [NSDate date];
            loadIcon([NSURL URLWithString:[origin stringByAppendingString:path]], ^(NSString *value) {
                dispatch_async(dispatch_get_main_queue(), ^{ image = value; done = YES; });
            });
            while (!done && -start.timeIntervalSinceNow < 5) {
                [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
            }
            assert(done);
            assert((image != nil) == [cases[path] boolValue]);
            if (image) assert([image hasPrefix:@"data:image/svg+xml;base64,"]);
            printf("PASS %s\n", path.UTF8String);
        }
    }
    return 0;
}
