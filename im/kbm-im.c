#define _GNU_SOURCE
#include <dlfcn.h>
#include <string.h>
#include <syslog.h>

#define ORIG "/usr/lib/gtk-2.0/2.10.0/immodules/libinput.so"
#define GDK_KEY_PRESS 8
#define GDK_RETURN 0xff0d
#define GDK_BACKSPACE 0xff08
#define GTK220_FILTER_KEYPRESS_SLOT (112 / sizeof(void *))

typedef struct {
    int type;
    void *window;
    signed char send_event;
    unsigned int time;
    unsigned int state;
    unsigned int keyval;
    int length;
    char *string;
    unsigned short hardware_keycode;
    unsigned char group;
    unsigned int is_modifier : 1;
} GdkEventKey;

typedef int (*filter_fn)(void *ctx, GdkEventKey *ev);

static void (*orig_init)(void *);
static void (*orig_exit)(void);
static void (*orig_list)(const void ***, int *);
static void *(*orig_create)(const char *);
static unsigned int (*to_unicode)(unsigned int);
static filter_fn orig_filter;

static int filter(void *ctx, GdkEventKey *ev)
{
    unsigned int kv = ev->keyval, u;

    if (ev->type != GDK_KEY_PRESS || kv == GDK_RETURN || kv == GDK_BACKSPACE)
        return orig_filter(ctx, ev);
    u = to_unicode(kv);
    if (u < 0x20 || u == 0x7f)
        return 0;
    if (u == kv)
        return orig_filter(ctx, ev);
    GdkEventKey copy = *ev;
    copy.keyval = u;
    return orig_filter(ctx, &copy);
}

static void hook(void *ctx)
{
    void **cls = *(void ***)ctx;
    void **slot = &cls[GTK220_FILTER_KEYPRESS_SLOT];
    Dl_info info;

    if (orig_filter || !to_unicode)
        return;
    if (!dladdr(*slot, &info) || !info.dli_fname || strcmp(info.dli_fname, ORIG)) {
        syslog(LOG_WARNING, "kbm-im: filter_keypress not where expected, leaving the input method alone");
        to_unicode = 0;
        return;
    }
    orig_filter = *slot;
    *slot = (void *)filter;
}

void im_module_init(void *module)
{
    void *orig = dlopen(ORIG, RTLD_NOW | RTLD_LOCAL);

    if (!orig) {
        syslog(LOG_ERR, "kbm-im: %s", dlerror());
        return;
    }
    orig_init = dlsym(orig, "im_module_init");
    orig_exit = dlsym(orig, "im_module_exit");
    orig_list = dlsym(orig, "im_module_list");
    orig_create = dlsym(orig, "im_module_create");
    to_unicode = dlsym(RTLD_DEFAULT, "gdk_keyval_to_unicode");
    if (orig_init)
        orig_init(module);
}

void im_module_exit(void)
{
    if (orig_exit)
        orig_exit();
}

void im_module_list(const void ***contexts, int *n)
{
    if (orig_list)
        orig_list(contexts, n);
    else
        *n = 0;
}

void *im_module_create(const char *id)
{
    void *ctx = orig_create ? orig_create(id) : 0;

    if (ctx)
        hook(ctx);
    return ctx;
}
