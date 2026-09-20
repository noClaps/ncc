/* Futures own their result storage until program teardown. Discarding a future
 * drops the handle; it does not abruptly cancel code or abandon held locks. */
static void nc_panic(const char *message);
static void *nc_alloc(size_t count, size_t size);
typedef struct nc_future {
    pthread_t thread;
    pthread_mutex_t join_lock;
    int joined;
    void *result;
    struct nc_future *next;
} nc_future;
static nc_future *nc_futures;
static pthread_mutex_t nc_futures_lock = PTHREAD_MUTEX_INITIALIZER;
static void nc_wait(nc_future *future) {
    pthread_mutex_lock(&future->join_lock);
    if (!future->joined) {
        if (pthread_join(future->thread, 0)) nc_panic("cannot await future");
        future->joined = 1;
    }
    pthread_mutex_unlock(&future->join_lock);
}
static void nc_start(nc_future *future, void *(*run)(void *), void *job) {
    if (pthread_mutex_init(&future->join_lock, 0)) nc_panic("cannot initialize future");
    pthread_mutex_lock(&nc_futures_lock);
    if (pthread_create(&future->thread, 0, run, job)) nc_panic("cannot start future");
    future->next = nc_futures;
    nc_futures = future;
    pthread_mutex_unlock(&nc_futures_lock);
}
static void nc_async_cleanup(void) {
    /* A worker may spawn more workers, so drain the live list, not a snapshot. */
    for (;;) {
        pthread_mutex_lock(&nc_futures_lock);
        nc_future *future = nc_futures;
        if (future) nc_futures = future->next;
        pthread_mutex_unlock(&nc_futures_lock);
        if (!future) break;
        nc_wait(future);
    }
}
