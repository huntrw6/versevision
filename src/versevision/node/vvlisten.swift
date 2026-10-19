import Foundation
import Speech
import AVFoundation
import CoreAudio

// vvlisten: stream what the selected audio input hears, as JSON lines on stdout.
//   vvlisten [--device "name part"] [--list] [--seconds N]
// Lines: {"t":secs,"kind":"partial"|"final"|"status","text":"..."}   Nothing is recorded to disk.
setvbuf(stdout, nil, _IOLBF, 0)
let args = Array(CommandLine.arguments.dropFirst())
func opt(_ n: String) -> String? { if let i = args.firstIndex(of: n), i + 1 < args.count { return args[i + 1] }; return nil }
let outPath = opt("--out")
var outHandle: FileHandle? = nil
if let o = outPath { FileManager.default.createFile(atPath: o, contents: nil); outHandle = FileHandle(forWritingAtPath: o) }
func emit(_ kind: String, _ text: String, _ t0: Date) {
    let o: [String: Any] = ["t": (Date().timeIntervalSince(t0) * 1000).rounded() / 1000, "kind": kind, "text": text]
    if let d = try? JSONSerialization.data(withJSONObject: o), let s = String(data: d, encoding: .utf8) {
        if let h = outHandle { h.write((s + "\n").data(using: .utf8)!) } else { print(s) }
    }
}

func inputDevices() -> [(id: AudioDeviceID, name: String)] {
    var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size)
    var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids)
    var out: [(AudioDeviceID, String)] = []
    for id in ids {
        var a = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreamConfiguration, mScope: kAudioDevicePropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var sz: UInt32 = 0
        if AudioObjectGetPropertyDataSize(id, &a, 0, nil, &sz) != noErr { continue }
        let buf = UnsafeMutableRawPointer.allocate(byteCount: Int(sz), alignment: 16); defer { buf.deallocate() }
        AudioObjectGetPropertyData(id, &a, 0, nil, &sz, buf)
        let abl = buf.assumingMemoryBound(to: AudioBufferList.self)
        let chans = UnsafeMutableAudioBufferListPointer(abl).reduce(0) { $0 + Int($1.mNumberChannels) }
        if chans == 0 { continue }
        var nameRef: Unmanaged<CFString>? = nil
        var na = AudioObjectPropertyAddress(mSelector: kAudioObjectPropertyName, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var nsz = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        AudioObjectGetPropertyData(id, &na, 0, nil, &nsz, &nameRef)
        out.append((id, (nameRef?.takeRetainedValue() as String?) ?? "?"))
    }
    return out
}
if args.contains("--list") { for d in inputDevices() { print(d.name) }; exit(0) }

let t0 = Date()
let sem = DispatchSemaphore(value: 0)
var speechOK = false
SFSpeechRecognizer.requestAuthorization { st in speechOK = (st == .authorized); sem.signal() }
emit("status", "waiting for Speech Recognition permission (click Allow on the screen if asked)", t0)
sem.wait()
if !speechOK { emit("status", "speech recognition permission not granted", t0); exit(2) }
var micOK = false
AVCaptureDevice.requestAccess(for: .audio) { ok in micOK = ok; sem.signal() }
emit("status", "waiting for Microphone permission (click Allow on the screen if asked)", t0)
sem.wait()
if !micOK { emit("status", "microphone permission not granted", t0); exit(3) }

guard let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US")), rec.isAvailable else { emit("status", "recognizer unavailable", t0); exit(4) }
rec.defaultTaskHint = .dictation
let engine = AVAudioEngine()

if let want = opt("--device")?.lowercased() {
    guard let dev = inputDevices().first(where: { $0.name.lowercased().contains(want) }) else { emit("status", "no input device matching '\(want)'", t0); exit(5) }
    var id = dev.id
    if let au = engine.inputNode.audioUnit {
        let st = AudioUnitSetProperty(au, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &id, UInt32(MemoryLayout<AudioDeviceID>.size))
        emit("status", st == noErr ? "input: \(dev.name)" : "could not select \(dev.name) (error \(st))", t0)
    }
} else { emit("status", "input: system default", t0) }

if let file = opt("--file") {            // recognise a recorded audio file instead of a live input (for testing)
    let url = URL(fileURLWithPath: file)
    let req = SFSpeechURLRecognitionRequest(url: url)
    req.shouldReportPartialResults = true
    req.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition
    req.addsPunctuation = false
    var last = ""
    rec.recognitionTask(with: req) { result, error in
        if let r = result {
            let t = r.bestTranscription.formattedString
            if r.isFinal { emit("final", t, t0); exit(0) } else if t != last { last = t; emit("partial", t, t0) }
        }
        if let e = error { emit("status", "error: \(e.localizedDescription)", t0); exit(7) }
    }
    RunLoop.main.run()
}
let fmt = engine.inputNode.outputFormat(forBus: 0)
var request: SFSpeechAudioBufferRecognitionRequest?
var task: SFSpeechRecognitionTask?
var lastPartial = ""
var generation = 0
var level: Float = 0

func startTask() {
    generation += 1
    let g = generation
    let r = SFSpeechAudioBufferRecognitionRequest()
    r.shouldReportPartialResults = true
    r.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition      // audio stays on this computer
    r.addsPunctuation = false
    r.contextualStrings = ["Genesis", "Exodus", "Leviticus", "Deuteronomy", "Psalms", "Psalm", "Proverbs", "Ecclesiastes", "Isaiah", "Jeremiah", "Ezekiel", "Matthew", "Romans", "Corinthians", "Galatians", "Ephesians", "Philippians", "Colossians", "Thessalonians", "Timothy", "Hebrews", "Revelation", "Philemon", "Habakkuk", "Zephaniah", "Nahum", "Obadiah", "Malachi", "Zechariah", "Haggai"]
    request = r; lastPartial = ""
    task = rec.recognitionTask(with: r) { result, error in
        if g != generation { return }
        if let res = result {
            let txt = res.bestTranscription.formattedString
            if res.isFinal { emit("final", txt, t0); lastPartial = "" }
            else if txt != lastPartial { lastPartial = txt; emit("partial", txt, t0) }
        }
        if error != nil || (result?.isFinal ?? false) { DispatchQueue.main.async { if g == generation { startTask() } } }
    }
}
engine.inputNode.installTap(onBus: 0, bufferSize: 2048, format: fmt) { buf, _ in
    request?.append(buf)
    if let ch = buf.floatChannelData?[0] { var m: Float = 0; for i in 0..<Int(buf.frameLength) { m = max(m, abs(ch[i])) }; level = max(level * 0.9, m) }
}
engine.prepare()
do { try engine.start() } catch { emit("status", "audio engine failed: \(error.localizedDescription)", t0); exit(6) }
emit("status", String(format: "listening (%.0f Hz, on-device: %@)", fmt.sampleRate, rec.supportsOnDeviceRecognition ? "yes" : "no"), t0)
startTask()
// a recognition task is limited to about a minute: restart it every 50 s, and report the input level so a dead feed is obvious
Timer.scheduledTimer(withTimeInterval: 50, repeats: true) { _ in request?.endAudio(); DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { startTask() } }
Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in emit("status", String(format: "level %.3f", level), t0) }
if let s = opt("--seconds"), let n = Double(s) { DispatchQueue.main.asyncAfter(deadline: .now() + n) { engine.stop(); exit(0) } }
RunLoop.main.run()
