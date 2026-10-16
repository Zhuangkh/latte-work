#import <WebKit/WebKit.h>
#import <objc/runtime.h>

static char siteDataGrantKey;
static NSString *siteDataHost(NSString *host) {
    return [host.lowercaseString stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"[]"]];
}
static BOOL siteDataMatchesHost(NSString *domain, NSString *host) {
    domain = siteDataHost(domain);
    host = siteDataHost(host);
    return domain.length && domain.length <= 253 &&
        ([domain isEqual:host] || [host hasSuffix:[@"." stringByAppendingString:domain]]);
}
static void siteDataFinish(NSString *directory, NSDictionary *result) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
    [data writeToFile:[directory stringByAppendingPathComponent:@"result.json"] atomically:YES];
}

void latteBrowserSiteData(WKWebView *view, NSString *directory, NSString *query) {
    NSDictionary *input = [NSJSONSerialization JSONObjectWithData:[query dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
    NSString *host = [input isKindOfClass:NSDictionary.class] ? input[@"host"] : nil;
    if (![host isKindOfClass:NSString.class] || ![siteDataHost(view.URL.host) isEqual:siteDataHost(host)]) {
        siteDataFinish(directory, @{@"error":@"页面已变化，请重新查看网站数据"}); return;
    }
    NSString *url = view.URL.absoluteString;
    WKWebsiteDataStore *store = view.configuration.websiteDataStore;
    NSString *token = input[@"token"];
    if (token) {
        NSDictionary *grant = objc_getAssociatedObject(view, &siteDataGrantKey);
        // Consume before clearing. It is bound to this surface, URL, store and
        // exact records the user saw, rather than looking up a new target.
        objc_setAssociatedObject(view, &siteDataGrantKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        if (![token isKindOfClass:NSString.class] || ![grant[@"token"] isEqual:token] ||
            ![grant[@"url"] isEqual:url] || grant[@"store"] != store ||
            [grant[@"expires"] doubleValue] <= NSProcessInfo.processInfo.systemUptime) {
            siteDataFinish(directory, @{@"error":@"网站数据已变化，请重新查看后再清除"}); return;
        }
        [store.httpCookieStore getAllCookies:^(NSArray<NSHTTPCookie *> *cookies) {
            if (![view.URL.absoluteString isEqual:url] || view.configuration.websiteDataStore != store || cookies.count > 4096) {
                siteDataFinish(directory, @{@"error":@"页面或网站数据已变化，请重新查看后再清除"}); return;
            }
            NSMutableArray *selectedCookies = [NSMutableArray new];
            for (NSHTTPCookie *cookie in cookies) {
                NSString *domain = [cookie.domain stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"."]];
                for (NSString *scope in grant[@"domains"]) if (siteDataMatchesHost(scope, domain)) {
                    [selectedCookies addObject:cookie]; break;
                }
            }
            if (selectedCookies.count > 512) {
                siteDataFinish(directory, @{@"error":@"网站数据数量较多，暂时无法清除"}); return;
            }
            [store removeDataOfTypes:grant[@"types"] forDataRecords:grant[@"records"] completionHandler:^{
                // WebKit's record removal may leave session cookies in the
                // HTTP cookie store. Explicitly finish deleting this scope.
                dispatch_group_t pending = dispatch_group_create();
                for (NSHTTPCookie *cookie in selectedCookies) {
                    dispatch_group_enter(pending);
                    [store.httpCookieStore deleteCookie:cookie completionHandler:^{ dispatch_group_leave(pending); }];
                }
                dispatch_group_notify(pending, dispatch_get_main_queue(), ^{
                    siteDataFinish(directory, @{@"url":url, @"cleared":@YES});
                });
            }];
        }];
        return;
    }
    objc_setAssociatedObject(view, &siteDataGrantKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    [store fetchDataRecordsOfTypes:WKWebsiteDataStore.allWebsiteDataTypes completionHandler:^(NSArray<WKWebsiteDataRecord *> *records) {
        if (![view.URL.absoluteString isEqual:url] || records.count > 4096) {
            siteDataFinish(directory, @{@"error":@"暂时无法读取当前网站数据，请重试"}); return;
        }
        NSMutableArray *selected = [NSMutableArray new];
        NSMutableSet *types = [NSMutableSet new];
        NSMutableSet *domains = [NSMutableSet new];
        for (WKWebsiteDataRecord *record in records) if (siteDataMatchesHost(record.displayName, host)) {
            [selected addObject:record];
            [types unionSet:record.dataTypes];
            [domains addObject:record.displayName.lowercaseString];
        }
        if (selected.count > 8 || types.count > 32) {
            siteDataFinish(directory, @{@"error":@"网站数据范围过大，请稍后重试"}); return;
        }
        NSString *nonce = NSUUID.UUID.UUIDString.lowercaseString;
        NSDictionary *grant = @{@"token":nonce, @"url":url, @"store":store,
            @"records":[selected copy], @"types":[types copy], @"domains":[domains.allObjects copy],
            @"expires":@(NSProcessInfo.processInfo.systemUptime + 120)};
        objc_setAssociatedObject(view, &siteDataGrantKey, grant, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        siteDataFinish(directory, @{@"url":url, @"token":nonce,
            @"domains":[domains.allObjects sortedArrayUsingSelector:@selector(compare:)],
            @"types":[types.allObjects sortedArrayUsingSelector:@selector(compare:)]});
    }];
}
