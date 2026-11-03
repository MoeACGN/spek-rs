/* Windows 10 rejects a subsystem-10 image whose SecurityCookie is null
   unless IMAGE_GUARD_SECURITY_COOKIE_UNUSED is set. MinGW's copy sets
   CF_INSTRUMENTED and leaves the cookie null, which becomes 0xc000007b.
   This object is linked directly so that archive member is not used. */
typedef unsigned int DWORD;
typedef unsigned long long ULONGLONG;

struct LoadConfigDirectory {
    DWORD Size;
    DWORD TimeDateStamp;
    unsigned short MajorVersion;
    unsigned short MinorVersion;
    DWORD GlobalFlagsClear;
    DWORD GlobalFlagsSet;
    DWORD CriticalSectionDefaultTimeout;
    ULONGLONG DeCommitFreeBlockThreshold;
    ULONGLONG DeCommitTotalFreeThreshold;
    ULONGLONG LockPrefixTable;
    ULONGLONG MaximumAllocationSize;
    ULONGLONG VirtualMemoryThreshold;
    ULONGLONG ProcessAffinityMask;
    DWORD ProcessHeapFlags;
    unsigned short CSDVersion;
    unsigned short DependentLoadFlags;
    ULONGLONG EditList;
    ULONGLONG SecurityCookie;
    ULONGLONG SEHandlerTable;
    ULONGLONG SEHandlerCount;
    ULONGLONG GuardCFCheckFunctionPointer;
    ULONGLONG GuardCFDispatchFunctionPointer;
    ULONGLONG GuardCFFunctionTable;
    ULONGLONG GuardCFFunctionCount;
    DWORD GuardFlags;
};

__attribute__((used))
const struct LoadConfigDirectory _load_config_used = {
    .Size = 0x94,
    .GuardFlags = 0x00000800,
};
