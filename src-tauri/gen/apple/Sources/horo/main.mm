#include "bindings/bindings.h"

#import <Photos/Photos.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>

/*
 * Remove the keyboard's input assistant bar — the strip carrying ⌃ ⌄ and Done
 * that iOS draws above the keyboard for any focused field in a web view.
 *
 * It is drawn by UIKit, not by the page, so nothing in CSS or JS can reach it.
 * The only lever is `inputAccessoryView` on the view that owns the text input,
 * which inside WKWebView is the private `WKContentView`. Replacing that method
 * with one returning nil is the standard fix, and is what every web-view
 * framework that offers a "hide accessory bar" switch does underneath.
 *
 * Two things to know before touching this:
 *
 * - `WKContentView` is private. We never call into it — we look it up by name
 *   and swap one method — but an iOS release could rename it, at which point
 *   `NSClassFromString` returns nil and the bar simply comes back. That is why
 *   every step below fails soft: a broken cosmetic patch must not be a crash.
 * - Apple has accepted this pattern for years, but it is still a private class
 *   name in a shipping binary. Worth a second look before submitting to the
 *   App Store.
 *
 * This lives in generated code. `tauri ios init` rewrites `gen/apple`, so if
 * the bar ever returns after regenerating the iOS project, it is this that went
 * missing.
 */
@interface HoroKeyboardAccessoryPatch : NSObject
@end

@implementation HoroKeyboardAccessoryPatch

+ (void)load {
  // WebKit's classes are registered when its image loads, which has not
  // necessarily happened at `+load` time. Defer to the first turn of the run
  // loop, by which point `start_app` has built the web view.
  dispatch_async(dispatch_get_main_queue(), ^{
    Class contentView = NSClassFromString(@"WKContentView");
    if (contentView == nil) {
      return;
    }

    SEL selector = @selector(inputAccessoryView);
    IMP none = imp_implementationWithBlock(^id(id _self) { return nil; });

    Method existing = class_getInstanceMethod(contentView, selector);
    if (existing != NULL) {
      method_setImplementation(existing, none);
    } else {
      // `@@:` — returns an object, takes self and _cmd.
      class_addMethod(contentView, selector, none, "@@:");
    }
  });
}

@end


/*
 * Photo library bridge.
 *
 * A web view cannot see the photo library: `<input type="file">` runs the
 * picker out of process and hands back only what the user chose, which is a
 * good privacy property and also why an in-app grid of recent photos has to be
 * native.
 *
 * Three C entry points, called from `photos.rs` on a Rust worker thread:
 *
 *   horo_photos_recent  — authorise, then JSON for the N newest items, each
 *                         with a small JPEG thumbnail as a data URI.
 *   horo_photos_export  — copy one asset's original bytes to a temp file and
 *                         return the path, so the existing path-based upload
 *                         can send it unchanged.
 *   horo_photos_free    — release a string from either of the above.
 *
 * Everything here is synchronous by design. PhotoKit is callback-driven, so
 * each call blocks on a semaphore; that is safe *only* because the caller is a
 * Tauri worker thread, never the main thread. Calling these from the main
 * thread would deadlock against PhotoKit's own main-queue callbacks.
 */

static char *horo_copy_cstr(NSString *string) {
  const char *utf8 = string.UTF8String;
  if (utf8 == NULL) {
    return NULL;
  }
  return strdup(utf8);
}

static char *horo_json(id object) {
  NSError *error = nil;
  NSData *data = [NSJSONSerialization dataWithJSONObject:object options:0 error:&error];
  if (data == nil) {
    return horo_copy_cstr(@"{\"status\":\"error\",\"message\":\"could not encode\"}");
  }
  return horo_copy_cstr([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]);
}

/** Ask once, block until answered. `limited` counts as usable — see below. */
static PHAuthorizationStatus horo_authorise(void) {
  PHAuthorizationStatus status =
      [PHPhotoLibrary authorizationStatusForAccessLevel:PHAccessLevelReadWrite];
  if (status != PHAuthorizationStatusNotDetermined) {
    return status;
  }

  __block PHAuthorizationStatus answered = PHAuthorizationStatusDenied;
  dispatch_semaphore_t done = dispatch_semaphore_create(0);
  [PHPhotoLibrary requestAuthorizationForAccessLevel:PHAccessLevelReadWrite
                                             handler:^(PHAuthorizationStatus granted) {
                                               answered = granted;
                                               dispatch_semaphore_signal(done);
                                             }];
  dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
  return answered;
}

extern "C" char *horo_photos_recent(int limit, int thumb_px) {
  @autoreleasepool {
    PHAuthorizationStatus status = horo_authorise();

    // `limited` is not a failure: the user picked some photos to share and we
    // see exactly those. Treating it as denied would be telling them their
    // choice didn't work.
    if (status != PHAuthorizationStatusAuthorized && status != PHAuthorizationStatusLimited) {
      return horo_json(@{@"status" : @"denied"});
    }

    PHFetchOptions *options = [PHFetchOptions new];
    options.sortDescriptors = @[ [NSSortDescriptor sortDescriptorWithKey:@"creationDate"
                                                              ascending:NO] ];
    options.predicate = [NSPredicate predicateWithFormat:@"mediaType == %d || mediaType == %d",
                                                         PHAssetMediaTypeImage,
                                                         PHAssetMediaTypeVideo];
    options.fetchLimit = limit > 0 ? limit : 30;

    PHFetchResult<PHAsset *> *assets = [PHAsset fetchAssetsWithOptions:options];

    PHImageRequestOptions *thumbOptions = [PHImageRequestOptions new];
    thumbOptions.synchronous = YES;
    thumbOptions.deliveryMode = PHImageRequestOptionsDeliveryModeFastFormat;
    thumbOptions.resizeMode = PHImageRequestOptionsResizeModeFast;
    // A thumbnail already in the library beats spinning up a network fetch for
    // one; an iCloud-only original is exported on demand instead.
    thumbOptions.networkAccessAllowed = NO;

    CGFloat side = thumb_px > 0 ? thumb_px : 160;
    NSMutableArray *out = [NSMutableArray arrayWithCapacity:assets.count];

    [assets enumerateObjectsUsingBlock:^(PHAsset *asset, NSUInteger idx, BOOL *stop) {
      __block NSString *thumb = nil;
      [[PHImageManager defaultManager] requestImageForAsset:asset
                                                 targetSize:CGSizeMake(side, side)
                                                contentMode:PHImageContentModeAspectFill
                                                    options:thumbOptions
                                              resultHandler:^(UIImage *image, NSDictionary *info) {
                                                if (image == nil) {
                                                  return;
                                                }
                                                NSData *jpeg = UIImageJPEGRepresentation(image, 0.6);
                                                if (jpeg == nil) {
                                                  return;
                                                }
                                                thumb = [@"data:image/jpeg;base64,"
                                                    stringByAppendingString:
                                                        [jpeg base64EncodedStringWithOptions:0]];
                                              }];

      if (thumb == nil) {
        return;
      }

      [out addObject:@{
        @"id" : asset.localIdentifier,
        @"video" : @(asset.mediaType == PHAssetMediaTypeVideo),
        @"seconds" : @((int)llround(asset.duration)),
        @"thumb" : thumb,
      }];
    }];

    return horo_json(@{
      @"status" : status == PHAuthorizationStatusLimited ? @"limited" : @"ok",
      @"assets" : out,
    });
  }
}

extern "C" char *horo_photos_export(const char *local_id) {
  @autoreleasepool {
    if (local_id == NULL) {
      return horo_json(@{@"status" : @"error", @"message" : @"no asset"});
    }

    NSString *identifier = [NSString stringWithUTF8String:local_id];
    PHAsset *asset = [PHAsset fetchAssetsWithLocalIdentifiers:@[ identifier ] options:nil].firstObject;
    if (asset == nil) {
      return horo_json(@{@"status" : @"error", @"message" : @"that photo is gone"});
    }

    // The *resource* rather than a rendered image: this is the file the user
    // actually has, at its original size and type, which is what they expect to
    // have sent. It covers video and stills through one path.
    PHAssetResource *resource = nil;
    for (PHAssetResource *candidate in [PHAssetResource assetResourcesForAsset:asset]) {
      if (candidate.type == PHAssetResourceTypePhoto ||
          candidate.type == PHAssetResourceTypeVideo) {
        resource = candidate;
        break;
      }
    }
    if (resource == nil) {
      return horo_json(@{@"status" : @"error", @"message" : @"couldn't read that photo"});
    }

    NSString *name = resource.originalFilename.length > 0 ? resource.originalFilename : @"photo";
    NSString *path = [NSTemporaryDirectory()
        stringByAppendingPathComponent:[NSString stringWithFormat:@"%@-%@",
                                                                 [[NSUUID UUID] UUIDString], name]];
    [[NSFileManager defaultManager] removeItemAtPath:path error:nil];

    PHAssetResourceRequestOptions *options = [PHAssetResourceRequestOptions new];
    // Unlike thumbnails, this one is worth waiting on the network for: the
    // original may live only in iCloud.
    options.networkAccessAllowed = YES;

    __block NSError *failure = nil;
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    [[PHAssetResourceManager defaultManager] writeDataForAssetResource:resource
                                                                toFile:[NSURL fileURLWithPath:path]
                                                               options:options
                                                     completionHandler:^(NSError *error) {
                                                       failure = error;
                                                       dispatch_semaphore_signal(done);
                                                     }];
    dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);

    if (failure != nil) {
      return horo_json(@{@"status" : @"error", @"message" : failure.localizedDescription});
    }
    return horo_json(@{@"status" : @"ok", @"path" : path});
  }
}

extern "C" void horo_photos_free(char *pointer) {
  free(pointer);
}

/*
 * Defined in `photos.rs`. Registration rather than direct calls, because the
 * Rust library is linked before this file's objects exist — see the long note
 * in `photos.rs` for why the dependency can only run this way.
 */
extern "C" void horo_register_photo_bridge(char *(*recent)(int, int),
                                           char *(*exporter)(const char *),
                                           void (*release)(char *));

int main(int argc, char * argv[]) {
	horo_register_photo_bridge(horo_photos_recent, horo_photos_export, horo_photos_free);
	ffi::start_app();
	return 0;
}
