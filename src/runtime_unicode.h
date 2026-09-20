static void nc_panic(const char *message);
static void *nc_alloc(size_t count, size_t size);
static unsigned nc_property(uint32_t c) {
    if (c >= 0xac00 && c <= 0xd7a3) return (c-0xac00)%28 ? 9 : 8;
    size_t lo=0, hi=sizeof(nc_unicode_ranges)/sizeof(*nc_unicode_ranges);
    while (lo<hi) { size_t mid=lo+(hi-lo)/2; if (nc_unicode_ranges[mid].hi<c) lo=mid+1; else hi=mid; }
    return lo<sizeof(nc_unicode_ranges)/sizeof(*nc_unicode_ranges) && nc_unicode_ranges[lo].lo<=c ? nc_unicode_ranges[lo].property : 0;
}
static const unsigned char *nc_utf8(const unsigned char *s, uint32_t *value) {
    uint32_t c=*s++; unsigned more=0, min=0;
    if (c>=0xf0 && c<=0xf4) {more=3;min=0x10000;c&=7;}
    else if (c>=0xe0 && c<=0xef) {more=2;min=0x800;c&=15;}
    else if (c>=0xc2 && c<=0xdf) {more=1;min=0x80;c&=31;}
    else if (c>=0x80) nc_panic("invalid UTF-8");
    while (more--) { if ((*s&0xc0)!=0x80) nc_panic("invalid UTF-8"); c=(c<<6)|(*s++&63); }
    if (c<min || c>0x10ffff || (c>=0xd800 && c<=0xdfff)) nc_panic("invalid UTF-8");
    *value=c;return s;
}
/* Category numbers match unicode.rs and the generated Unicode property data. */
static const char *nc_grapheme_next(const char *text) {
    const unsigned char *s=(const unsigned char*)text;
    unsigned previous=0, started=0, regional=0, emoji=0, zwj=0, consonant=0, linker=0;
    while (*s) {
        uint32_t c; const unsigned char *next=nc_utf8(s,&c);
        unsigned property=nc_property(c), current=property&15, boundary;
        if (!started || (previous==1 && current==7)) boundary=0;
        else if (previous==1 || previous==2 || previous==7 || current==1 || current==2 || current==7) boundary=1;
        else boundary=!((previous==6 && (current==6 || current==8 || current==9 || current==14))
            || ((previous==8 || previous==14) && (current==13 || current==14))
            || ((previous==9 || previous==13) && current==13)
            || current==3 || current==12 || current==15 || previous==10
            || (current==5 && consonant && linker)
            || (previous==15 && current==4 && zwj)
            || (previous==11 && current==11 && regional%2));
        if (boundary) return (const char*)s;
        regional=current==11 ? regional+1 : 0;
        zwj=current==15 && emoji; emoji=current==4 || (current==3 && emoji);
        if (current==5) {consonant=1;linker=0;}
        else if (c==0x94d || c==0x9cd || c==0xacd || c==0xb4d || c==0xc4d || c==0xd4d) linker=1;
        else if (!(property&16)) {consonant=0;linker=0;}
        previous=current;started=1;s=next;
    }
    return (const char*)s;
}
static uint64_t nc_str_len(const char *text) {
    uint64_t length=0; while (*text) {text=nc_grapheme_next(text);++length;} return length;
}
static const char *nc_str_at(const char *text, uint64_t index) {
    while (*text && index) {text=nc_grapheme_next(text);--index;}
    if (!*text || index) nc_panic("string index out of bounds");
    return text;
}
static const char *nc_str_index(const char *text, uint64_t index) {
    const char *start=nc_str_at(text,index), *end=nc_grapheme_next(start);
    size_t length=(size_t)(end-start); char *value=nc_alloc(length+1,1);
    memcpy(value,start,length); return value;
}
static const char *nc_str_replace(const char *text, uint64_t index, const char *value) {
    const char *start=nc_str_at(text,index), *end=nc_grapheme_next(start);
    size_t prefix=(size_t)(start-text), middle=strlen(value), suffix=strlen(end);
    if (middle>SIZE_MAX-prefix || suffix>=SIZE_MAX-prefix-middle) nc_panic("string length overflow");
    char *result=nc_alloc(prefix+middle+suffix+1,1);
    memcpy(result,text,prefix);memcpy(result+prefix,value,middle);memcpy(result+prefix+middle,end,suffix);
    return result;
}
