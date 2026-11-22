// The scsynth side of the reference comparison: runs a small UGen graph
// through SuperCollider's own plugin code and prints what one of its wires
// carried, so `tests/offline/sc_reference.rs` can hold the server's UGens
// against it.
//
// This is not scsynth. It is the part of scsynth a plugin sees -- a World, a
// Graph, its Units and Wires, the interface table -- built the way
// `SC_Graph.cpp` builds them: every unit constructed in graph order on the
// first block, then the calc units (control and audio rate) run once per
// block, an audio-rate wire one block long and every other wire a scalar.
// The plugins themselves are SuperCollider's, compiled from a checkout and
// loaded with `dlopen` exactly as scsynth loads them, so what this prints is
// what scsynth would compute. `gen.py` builds it and drives it.
//
// Input, one command per line (whitespace separated):
//
//   case NAME BLOCKS SEED SR   start a graph; SEED seeds every generator, SR
//                              is the sample rate
//   buf NUM CHANNELS FRAMES SR v0 v1 ...   fill buffer NUM (interleaved)
//   ugen KIND RATE SPECIAL NOUT NIN IN...  RATE is ir|kr|ar|dr; an IN is
//                              c:VALUE (a constant) or u:UNIT:OUTPUT
//   probe UNIT OUTPUT          the wire to print
//   run                        build, run BLOCKS blocks, print, discard
//
// Output, one line per probe: `= NAME v v v ...` -- one value per sample for an
// audio-rate wire and one per block for the rest -- then `= NAME done B` if
// the graph freed itself at the end of block B. The `=` sets them apart from
// whatever a plugin prints on its own.

#include <dlfcn.h>
#include <glob.h>

#include <cmath>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <map>
#include <sstream>
#include <string>
#include <vector>

#include "SC_Graph.h"
#include "SC_InterfaceTable.h"
#include "SC_RGen.h"
#include "SC_Rate.h"
#include "SC_SndBuf.h"
#include "SC_Unit.h"
#include "SC_Wire.h"
#include "SC_World.h"
// The FFT the plugins call through the table, from common/SC_fftlib.cpp;
// its header undoes the table's macros of the same names.
#include "SC_fftlib.hpp"

// From SuperCollider's common/Samp.cpp, linked in.
extern float32 gSine[];
extern float32 gInvSine[];
extern float32 gSineWavetable[];

const int kSineSizeHarness = 8192;
const int kBlock = 64;
const int kAudioBuses = 128;
const int kControlBuses = 4096;
const int kBuffers = 64;
const int kRGens = 64;

struct UnitDef {
    std::string name;
    size_t size;
    UnitCtorFunc ctor;
    UnitDtorFunc dtor;
    uint32 flags;
};

static std::map<std::string, UnitDef> gDefs;
static InterfaceTable gTable;

// What the graph asked of its node this block.
static bool gFreed = false;

static int32 print(const char* fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    return 0;
}

static int32 ranSeed() { return 0x1234; }

static SCBool defineUnit(const char* name, size_t size, UnitCtorFunc ctor, UnitDtorFunc dtor, uint32 flags) {
    gDefs[name] = UnitDef { name, size, ctor, dtor, flags };
    return true;
}

static SCBool definePlugInCmd(const char*, PlugInCmdFunc, void*) { return true; }
static SCBool defineUnitCmd(const char*, const char*, UnitCmdFunc) { return true; }
static SCBool defineUnitCmdEx(const char*, const char*, UnitCmdFuncEx) { return true; }
static SCBool defineBufGen(const char*, BufGenFunc) { return true; }

static void clearUnitOutputs(Unit* unit, int32 n) {
    for (uint32 i = 0; i < unit->mNumOutputs; ++i)
        memset(unit->mOutBuf[i], 0, n * sizeof(float));
}

static void* rtAlloc(World*, size_t size) { return calloc(1, size ? size : 1); }
static void* rtRealloc(World*, void* p, size_t size) { return realloc(p, size); }
static void rtFree(World*, void* p) { free(p); }

static void nodeRun(Node*, int32 run) {
    if (!run)
        fprintf(stderr, "harness: a unit paused its node\n");
}
static void nodeEnd(Node*) { gFreed = true; }
static void sendTrigger(Node*, int32, float) {}
static void sendNodeReply(Node*, int32, const char*, int, const float*) {}
static SCBool sendMsg(World*, FifoMsg*) { return false; }
static int formatInfo(SF_INFO*, const char*, const char*) { return 1; }
static Node* getNode(World*, int32) { return nullptr; }
static Graph* getGraph(World*, int32) { return nullptr; }
static void nrtLock(World*) {}
static void groupDeleteAll(Group*) {}

static void doneAction(int32 action, Unit*) {
    // Every action that frees the synth itself frees it here: the graph has
    // no siblings, so the actions that differ only in what else they free
    // are the same action.
    switch (action) {
    case 2:
    case 3:
    case 4:
    case 5:
    case 6:
    case 7:
    case 8:
    case 13:
    case 14:
        gFreed = true;
        break;
    default:
        break;
    }
}

static SCErr doAsync(World*, void*, const char*, void*, AsyncStageFn, AsyncStageFn, AsyncStageFn, AsyncFreeFn,
                     int32, const void*) {
    return 1;
}
static SCErr doAsyncEx(World*, void*, const char*, void*, AsyncStageFnEx, AsyncStageFnEx, AsyncStageFnEx,
                       AsyncFreeFn, int32, const void*) {
    return 1;
}
static SCErr doAsyncUnit(Unit*, void*, const char*, void*, AsyncUnitStageFn, AsyncUnitStageFn, AsyncUnitStageFn,
                         AsyncFreeFn, int32, const void*) {
    return 1;
}

static SCErr bufAlloc(SndBuf* buf, int32 channels, int32 frames, double sr) {
    long samples = (long)channels * frames;
    buf->data = (float*)calloc(samples + 4, sizeof(float));
    buf->channels = channels;
    buf->frames = frames;
    buf->samples = (int32)samples;
    buf->samplerate = sr;
    buf->sampledur = 1.0 / sr;
    int32 mask = 1;
    while (mask < samples)
        mask <<= 1;
    buf->mask = mask - 1;
    buf->mask1 = buf->mask - 1;
    buf->coord = 0;
    return 0;
}

static SCBool getScope(World*, int32, int32, int32, ScopeBufferHnd*) { return false; }
static void pushScope(World*, ScopeBufferHnd*, int) {}
static void releaseScope(World*, ScopeBufferHnd*) {}

static void initTable() {
    InterfaceTable* ft = &gTable;
    memset(ft, 0, sizeof(*ft));
    ft->mSine = gSine;
    ft->mCosecant = gInvSine;
    ft->mSineSize = kSineSizeHarness;
    ft->mSineWavetable = gSineWavetable;
    ft->fPrint = &print;
    ft->fRanSeed = &ranSeed;
    ft->fNodeEnd = &nodeEnd;
    ft->fDefineUnit = &defineUnit;
    ft->fDefineBufGen = &defineBufGen;
    ft->fClearUnitOutputs = &clearUnitOutputs;
    ft->fNRTAlloc = &malloc;
    ft->fNRTRealloc = &realloc;
    ft->fNRTFree = &free;
    ft->fRTAlloc = &rtAlloc;
    ft->fRTRealloc = &rtRealloc;
    ft->fRTFree = &rtFree;
    ft->fNodeRun = &nodeRun;
    ft->fSendTrigger = &sendTrigger;
    ft->fSendNodeReply = &sendNodeReply;
    ft->fDefineUnitCmd = &defineUnitCmd;
    ft->fDefineUnitCmdEx = &defineUnitCmdEx;
    ft->fDefinePlugInCmd = &definePlugInCmd;
    ft->fSendMsgFromRT = &sendMsg;
    ft->fSendMsgToRT = &sendMsg;
    ft->fSndFileFormatInfoFromStrings = &formatInfo;
    ft->fGetNode = &getNode;
    ft->fGetGraph = &getGraph;
    ft->fNRTLock = &nrtLock;
    ft->fNRTUnlock = &nrtLock;
    ft->fGroup_DeleteAll = &groupDeleteAll;
    ft->fDoneAction = &doneAction;
    ft->fDoAsynchronousCommand = &doAsync;
    ft->fDoAsynchronousCommandEx = &doAsyncEx;
    ft->fDoAsyncUnitCommand = &doAsyncUnit;
    ft->fBufAlloc = &bufAlloc;
    ft->fSCfftCreate = &scfft_create;
    ft->fSCfftDestroy = &scfft_destroy;
    ft->fSCfftDoFFT = &scfft_dofft;
    ft->fSCfftDoIFFT = &scfft_doifft;
    ft->fGetScopeBuffer = &getScope;
    ft->fPushScopeBuffer = &pushScope;
    ft->fReleaseScopeBuffer = &releaseScope;
}

static void rateInit(Rate* r, double sr, int len) {
    r->mSampleRate = sr;
    r->mSampleDur = 1. / sr;
    r->mRadiansPerSample = 2 * M_PI / sr;
    r->mBufLength = len;
    r->mBufDuration = len / sr;
    r->mBufRate = sr / len;
    r->mSlopeFactor = 1. / len;
    r->mFilterLoops = len / 3;
    r->mFilterRemain = len % 3;
    r->mFilterSlope = r->mFilterLoops == 0 ? 0. : 1. / r->mFilterLoops;
}

static void loadPlugins(const char* dir) {
    glob_t g;
    std::string pattern = std::string(dir) + "/*.so";
    if (glob(pattern.c_str(), 0, nullptr, &g) != 0) {
        fprintf(stderr, "harness: no plugins in %s\n", dir);
        exit(1);
    }
    for (size_t i = 0; i < g.gl_pathc; ++i) {
        void* h = dlopen(g.gl_pathv[i], RTLD_NOW | RTLD_LOCAL);
        if (!h) {
            fprintf(stderr, "harness: %s\n", dlerror());
            exit(1);
        }
        auto load = (void (*)(InterfaceTable*))dlsym(h, "load");
        if (!load) {
            fprintf(stderr, "harness: %s has no load()\n", g.gl_pathv[i]);
            exit(1);
        }
        load(&gTable);
    }
    globfree(&g);
}

struct InSpec {
    bool constant;
    float value;
    int unit, output;
};

struct USpec {
    std::string kind;
    int rate;
    int special;
    int outputs;
    std::vector<InSpec> inputs;
};

struct BufSpec {
    int num, channels, frames;
    double sr;
    std::vector<float> data;
};

static int parseRate(const std::string& r) {
    if (r == "ir")
        return calc_ScalarRate;
    if (r == "kr")
        return calc_BufRate;
    if (r == "ar")
        return calc_FullRate;
    if (r == "dr")
        return calc_DemandRate;
    fprintf(stderr, "harness: unknown rate %s\n", r.c_str());
    exit(1);
}

static void run(const std::string& name, int blocks, uint32 seed, double sampleRate, const std::vector<USpec>& specs,
                const std::vector<BufSpec>& bufs, const std::vector<std::pair<int, int>>& probes) {
    World world;
    memset(&world, 0, sizeof(world));
    world.ft = &gTable;
    world.mSampleRate = sampleRate;
    world.mBufLength = kBlock;
    world.mNumAudioBusChannels = kAudioBuses;
    world.mNumControlBusChannels = kControlBuses;
    world.mNumInputs = 2;
    world.mNumOutputs = 2;
    world.mAudioBus = (float*)calloc(kAudioBuses * kBlock, sizeof(float));
    world.mControlBus = (float*)calloc(kControlBuses, sizeof(float));
    world.mAudioBusTouched = (int32*)calloc(kAudioBuses, sizeof(int32));
    world.mControlBusTouched = (int32*)calloc(kControlBuses, sizeof(int32));
    for (int i = 0; i < kAudioBuses; ++i)
        world.mAudioBusTouched[i] = -1;
    for (int i = 0; i < kControlBuses; ++i)
        world.mControlBusTouched[i] = -1;
    world.mNumSndBufs = kBuffers;
    world.mSndBufs = (SndBuf*)calloc(kBuffers, sizeof(SndBuf));
    world.mSndBufsNonRealTimeMirror = world.mSndBufs;
    world.mSndBufUpdates = (SndBufUpdates*)calloc(kBuffers, sizeof(SndBufUpdates));
    rateInit(&world.mFullRate, sampleRate, kBlock);
    rateInit(&world.mBufRate, sampleRate / kBlock, 1);
    world.mNumRGens = kRGens;
    world.mRGen = new RGen[kRGens];
    for (int i = 0; i < kRGens; ++i)
        world.mRGen[i].init(seed + i);
    world.mRealTime = false;
    world.mRunning = true;

    for (const BufSpec& b : bufs) {
        SndBuf* buf = world.mSndBufs + b.num;
        bufAlloc(buf, b.channels, b.frames, b.sr);
        memcpy(buf->data, b.data.data(), std::min<size_t>(b.data.size(), buf->samples) * sizeof(float));
    }

    Graph graph;
    memset(&graph, 0, sizeof(graph));
    graph.mNode.mID = 1000;
    graph.mNode.mWorld = &world;
    graph.mFullRate = &world.mFullRate;
    graph.mBufRate = &world.mBufRate;
    graph.mRGen = world.mRGen;
    graph.mNumTicks = 1;

    std::vector<Unit*> units;
    std::vector<std::vector<Wire*>> outWires;
    std::vector<Wire*> constWires;
    std::vector<Unit*> calc;
    for (size_t i = 0; i < specs.size(); ++i) {
        const USpec& s = specs[i];
        auto it = gDefs.find(s.kind);
        if (it == gDefs.end()) {
            fprintf(stderr, "harness: %s: no unit named %s\n", name.c_str(), s.kind.c_str());
            exit(1);
        }
        UnitDef* def = &it->second;
        Unit* unit = (Unit*)calloc(1, def->size);
        unit->mWorld = &world;
        unit->mUnitDef = def;
        unit->mParent = &graph;
        unit->mParentIndex = (int16)i;
        unit->mNumInputs = (uint32)s.inputs.size();
        unit->mNumOutputs = (uint32)s.outputs;
        size_t ports = s.inputs.size() + s.outputs;
        unit->mInput = (Wire**)calloc(ports + 1, sizeof(Wire*));
        unit->mOutput = unit->mInput + s.inputs.size();
        unit->mInBuf = (float**)calloc(ports + 1, sizeof(float*));
        unit->mOutBuf = unit->mInBuf + s.inputs.size();
        unit->mCalcRate = (int16)s.rate;
        unit->mSpecialIndex = (int16)s.special;
        unit->mRate = s.rate == calc_FullRate ? graph.mFullRate : graph.mBufRate;
        unit->mBufLength = unit->mRate->mBufLength;
        for (size_t j = 0; j < s.inputs.size(); ++j) {
            const InSpec& in = s.inputs[j];
            Wire* w;
            if (in.constant) {
                w = (Wire*)calloc(1, sizeof(Wire));
                w->mCalcRate = calc_ScalarRate;
                w->mScalarValue = in.value;
                w->mBuffer = &w->mScalarValue;
                constWires.push_back(w);
            } else {
                if (in.unit < 0 || (size_t)in.unit >= i || in.output >= (int)outWires[in.unit].size()) {
                    fprintf(stderr, "harness: %s: unit %zu reads a wire that is not before it\n", name.c_str(), i);
                    exit(1);
                }
                w = outWires[in.unit][in.output];
            }
            unit->mInput[j] = w;
            unit->mInBuf[j] = w->mBuffer;
        }
        std::vector<Wire*> outs;
        for (int j = 0; j < s.outputs; ++j) {
            Wire* w = (Wire*)calloc(1, sizeof(Wire));
            w->mFromUnit = unit;
            w->mCalcRate = s.rate;
            if (s.rate == calc_FullRate)
                w->mBuffer = (float*)calloc(kBlock, sizeof(float));
            else
                w->mBuffer = &w->mScalarValue;
            unit->mOutput[j] = w;
            unit->mOutBuf[j] = w->mBuffer;
            outs.push_back(w);
        }
        outWires.push_back(outs);
        units.push_back(unit);
        if (s.rate == calc_FullRate || s.rate == calc_BufRate)
            calc.push_back(unit);
    }

    std::vector<std::vector<float>> got(probes.size());
    gFreed = false;
    int freedAt = -1;
    for (int b = 0; b < blocks; ++b) {
        world.mBufCounter = b;
        if (freedAt < 0) {
            if (b == 0)
                for (Unit* u : units)
                    ((UnitDef*)u->mUnitDef)->ctor(u);
            for (Unit* u : calc)
                (u->mCalcFunc)(u, u->mBufLength);
            if (gFreed)
                freedAt = b;
        }
        for (size_t p = 0; p < probes.size(); ++p) {
            Wire* w = outWires[probes[p].first][probes[p].second];
            bool silent = freedAt >= 0 && b > freedAt;
            int n = w->mCalcRate == calc_FullRate ? kBlock : 1;
            for (int k = 0; k < n; ++k)
                got[p].push_back(silent ? 0.f : w->mBuffer[k]);
        }
    }
    for (size_t p = 0; p < probes.size(); ++p) {
        printf("= %s", name.c_str());
        for (float v : got[p])
            printf(" %.9g", v);
        printf("\n");
    }
    if (freedAt >= 0)
        printf("= %s done %d\n", name.c_str(), freedAt);
    fflush(stdout);

    for (Unit* u : units)
        if (((UnitDef*)u->mUnitDef)->dtor)
            ((UnitDef*)u->mUnitDef)->dtor(u);
}

int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: harness PLUGIN_DIR < cases\n");
        return 2;
    }
    initTable();
    loadPlugins(argv[1]);

    std::string line, name;
    int blocks = 0;
    uint32 seed = 0;
    double sr = 48000.0;
    std::vector<USpec> specs;
    std::vector<BufSpec> bufs;
    std::vector<std::pair<int, int>> probes;
    while (std::getline(std::cin, line)) {
        std::istringstream in(line);
        std::string cmd;
        if (!(in >> cmd))
            continue;
        if (cmd == "case") {
            in >> name >> blocks >> seed >> sr;
            specs.clear();
            bufs.clear();
            probes.clear();
        } else if (cmd == "buf") {
            BufSpec b;
            in >> b.num >> b.channels >> b.frames >> b.sr;
            float v;
            while (in >> v)
                b.data.push_back(v);
            bufs.push_back(b);
        } else if (cmd == "ugen") {
            USpec s;
            std::string rate;
            int nin;
            in >> s.kind >> rate >> s.special >> s.outputs >> nin;
            s.rate = parseRate(rate);
            for (int k = 0; k < nin; ++k) {
                std::string tok;
                in >> tok;
                InSpec is { false, 0.f, -1, 0 };
                if (tok.rfind("c:", 0) == 0) {
                    is.constant = true;
                    is.value = strtof(tok.c_str() + 2, nullptr);
                } else if (tok.rfind("u:", 0) == 0) {
                    sscanf(tok.c_str() + 2, "%d:%d", &is.unit, &is.output);
                } else {
                    fprintf(stderr, "harness: %s: bad input %s\n", name.c_str(), tok.c_str());
                    return 1;
                }
                s.inputs.push_back(is);
            }
            specs.push_back(s);
        } else if (cmd == "probe") {
            int u, o;
            in >> u >> o;
            probes.push_back({ u, o });
        } else if (cmd == "run") {
            run(name, blocks, seed, sr, specs, bufs, probes);
        } else if (cmd == "list") {
            for (auto& d : gDefs)
                printf("%s\n", d.first.c_str());
        } else {
            fprintf(stderr, "harness: unknown command %s\n", cmd.c_str());
            return 1;
        }
    }
    return 0;
}
