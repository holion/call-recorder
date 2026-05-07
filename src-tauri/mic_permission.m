#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <dispatch/dispatch.h>

// Returns: 0=NotDetermined, 1=Restricted, 2=Denied, 3=Authorized
int microphone_authorization_status(void) {
    return (int)[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
}

// Shows the system microphone permission dialog if not yet determined.
// Blocks until the user responds (or returns immediately if already decided).
// Returns 1 if authorized, 0 otherwise.
int request_microphone_access_sync(void) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block BOOL granted = NO;
    dispatch_async(dispatch_get_main_queue(), ^{
        [NSApp activateIgnoringOtherApps:YES];
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL g) {
            granted = g;
            dispatch_semaphore_signal(sem);
        }];
    });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    return granted ? 1 : 0;
}
