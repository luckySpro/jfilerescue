// JFileRescue macOS privileged helper (SMAppService daemon).
//
// Responsibility is deliberately tiny: on request from the signed GUI, open a
// raw disk device read-only and hand the file descriptor back over an XPC
// connection. It never writes, never mounts, never runs arbitrary paths.
//
// Build: part of `scripts/package-mac.sh` (not yet wired into tauri build);
// installed to /Library/PrivilegedHelperTools/com.mjgroup.jfilerescue.helper
// and registered via SMAppService with Contents/Library/LaunchDaemons/
// com.mjgroup.jfilerescue.helper.plist.
//
// Security checks before honouring a request:
//   1. Peer code signature: same Team ID + bundle id com.mjgroup.jfilerescue,
//      verified with SecCodeCopyGuestWithAttributes + SecCodeCheckValidity.
//   2. Requested path must match ^/dev/r?disk[0-9]+(s[0-9]+)*$ and resolve to
//      a character/block device (no symlinks, no files).
//   3. Opened with O_RDONLY | O_CLOEXEC only; fd passed via xpc_fd_create.
//   4. Version string is returned so the GUI can refuse mismatched helpers.

#import <Foundation/Foundation.h>
#import <Security/Security.h>
#include <xpc/xpc.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <regex.h>

#define HELPER_VERSION "0.1.0"
#define MACH_SERVICE "com.mjgroup.jfilerescue.helper"
#define REQUIREMENT "anchor apple generic and identifier \"com.mjgroup.jfilerescue\" and certificate leaf[subject.OU] = \"REPLACE_TEAM_ID\""

static bool peer_is_trusted(xpc_connection_t peer) {
    audit_token_t token;
    xpc_connection_get_audit_token(peer, &token);
    CFDataRef tokenData = CFDataCreate(NULL, (const UInt8 *)&token, sizeof(token));
    CFDictionaryRef attrs = CFDictionaryCreate(NULL, (const void **)&kSecGuestAttributeAudit, (const void **)&tokenData, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    SecCodeRef code = NULL;
    bool ok = false;
    if (SecCodeCopyGuestWithAttributes(NULL, attrs, kSecCSDefaultFlags, &code) == errSecSuccess) {
        SecRequirementRef req = NULL;
        if (SecRequirementCreateWithString(CFSTR(REQUIREMENT), kSecCSDefaultFlags, &req) == errSecSuccess) {
            ok = SecCodeCheckValidity(code, kSecCSDefaultFlags, req) == errSecSuccess;
            CFRelease(req);
        }
        CFRelease(code);
    }
    CFRelease(attrs);
    CFRelease(tokenData);
    return ok;
}

static bool path_is_raw_disk(const char *path) {
    regex_t re;
    if (regcomp(&re, "^/dev/r?disk[0-9]+(s[0-9]+)*$", REG_EXTENDED | REG_NOSUB) != 0) return false;
    bool m = regexec(&re, path, 0, NULL, 0) == 0;
    regfree(&re);
    if (!m) return false;
    struct stat st;
    if (lstat(path, &st) != 0) return false;
    return S_ISCHR(st.st_mode) || S_ISBLK(st.st_mode);
}

static void handle(xpc_connection_t peer, xpc_object_t msg) {
    xpc_object_t reply = xpc_dictionary_create_reply(msg);
    const char *op = xpc_dictionary_get_string(msg, "op");
    xpc_dictionary_set_string(reply, "version", HELPER_VERSION);
    if (!op) {
        xpc_dictionary_set_string(reply, "error", "missing op");
    } else if (strcmp(op, "version") == 0) {
        // version already set
    } else if (!peer_is_trusted(peer)) {
        xpc_dictionary_set_string(reply, "error", "caller not trusted");
    } else if (strcmp(op, "open_ro") == 0) {
        const char *path = xpc_dictionary_get_string(msg, "path");
        if (!path || !path_is_raw_disk(path)) {
            xpc_dictionary_set_string(reply, "error", "path not allowed");
        } else {
            int fd = open(path, O_RDONLY | O_CLOEXEC);
            if (fd < 0) {
                xpc_dictionary_set_int64(reply, "errno", errno);
                xpc_dictionary_set_string(reply, "error", strerror(errno));
            } else {
                xpc_dictionary_set_fd(reply, "fd", fd);
                close(fd);
            }
        }
    } else {
        xpc_dictionary_set_string(reply, "error", "unknown op");
    }
    xpc_connection_send_message(peer, reply);
}

int main(void) {
    xpc_connection_t listener = xpc_connection_create_mach_service(MACH_SERVICE, NULL, XPC_CONNECTION_MACH_SERVICE_LISTENER);
    xpc_connection_set_event_handler(listener, ^(xpc_object_t peer) {
        if (xpc_get_type(peer) != XPC_TYPE_CONNECTION) return;
        xpc_connection_set_event_handler(peer, ^(xpc_object_t msg) {
            if (xpc_get_type(msg) == XPC_TYPE_DICTIONARY) handle(peer, msg);
        });
        xpc_connection_resume(peer);
    });
    xpc_connection_resume(listener);
    dispatch_main();
}
