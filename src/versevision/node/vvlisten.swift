import Foundation
import Speech
import AVFoundation
import CoreAudio

// vvlisten: stream what the selected audio input hears, as JSON lines.
//   vvlisten --engine whisper --whisper http://127.0.0.1:8178 [--device "name part"] [--out file] [--seconds N] [--prompt "..."]
//   vvlisten --engine apple   [--device ...]        (Apple's on-device recognizer; needs Dictation on)
//   vvlisten --list
// Lines: {"t":secs,"kind":"partial"|"final"|"status","text":"..."}   Audio is never written to disk.
setvbuf(stdout, nil, _IOLBF, 0)
let args = Array(CommandLine.arguments.dropFirst())
func opt(_ n: String) -> String? { if let i = args.firstIndex(of: n), i + 1 < args.count { return args[i + 1] }; return nil }
let outPath = opt("--out")
var outHandle: FileHandle? = nil
if let o = outPath { FileManager.default.createFile(atPath: o, contents: nil); outHandle = FileHandle(forWritingAtPath: o) }
let emitLock = NSLock()
func emit(_ kind: String, _ text: String, _ t0: Date) {
    emitLock.lock(); defer { emitLock.unlock() }
    let o: [String: Any] = ["t": (Date().timeIntervalSince(t0) * 1000).rounded() / 1000, "w": (Date().timeIntervalSince1970 * 1000).rounded() / 1000, "kind": kind, "text": text]
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
let engineName = opt("--engine") ?? "apple"
let sem = DispatchSemaphore(value: 0)

if engineName == "apple" {
    var speechOK = false
    SFSpeechRecognizer.requestAuthorization { st in speechOK = (st == .authorized); sem.signal() }
    emit("status", "waiting for Speech Recognition permission (click Allow on the screen if asked)", t0)
    sem.wait()
    if !speechOK { emit("status", "speech recognition permission not granted", t0); exit(2) }
}
var micOK = false
AVCaptureDevice.requestAccess(for: .audio) { ok in micOK = ok; sem.signal() }
emit("status", "checking Microphone permission (an Allow box appears only the first time)", t0)
sem.wait()
if !micOK { emit("status", "microphone permission not granted", t0); exit(3) }

let engine = AVAudioEngine()
if let want = opt("--device")?.lowercased() {
    guard let dev = inputDevices().first(where: { $0.name.lowercased().contains(want) }) else { emit("status", "no input device matching '\(want)'", t0); exit(5) }
    var id = dev.id
    if let au = engine.inputNode.audioUnit {
        let st = AudioUnitSetProperty(au, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &id, UInt32(MemoryLayout<AudioDeviceID>.size))
        emit("status", st == noErr ? "input: \(dev.name)" : "could not select \(dev.name) (error \(st))", t0)
    }
} else { emit("status", "input: system default", t0) }
let fmt = engine.inputNode.outputFormat(forBus: 0)
var level: Float = 0

// ======================================================================= Whisper engine
// Cuts the live audio into utterances (speech between pauses), re-sends the growing utterance to a local
// whisper.cpp server about once a second for partial text, and sends the finished utterance once for the final text.
final class WhisperListener {
    let url: URL
    let prompt: String
    let rate = 16000
    var samples: [Float] = []                  // 16 kHz mono; trimmed as we go
    var base = 0                               // absolute index of samples[0]
    let lock = NSLock()
    var floorLevel: Float = 0.004
    var inUtterance = false
    var utterStart = 0
    var lastSpeechAt = Date.distantPast
    var lastRequestAt = Date.distantPast
    var inFlight = false
    var lastText = ""
    var prevEnd = -1                           // absolute index where the previous utterance ended
    let work = DispatchQueue(label: "whisper.work")
    let t0: Date
    let stepSeconds: Double
    let silenceSeconds: Double
    let maxUtterance: Double
    let overlapSeconds = 3.0

    init(url: URL, prompt: String, t0: Date, step: Double, silence: Double, maxUtter: Double) {
        self.url = url; self.prompt = prompt; self.t0 = t0; stepSeconds = step; silenceSeconds = silence; maxUtterance = maxUtter
    }
    var total: Int { base + samples.count }

    func append(_ chunk: [Float]) { lock.lock(); samples.append(contentsOf: chunk); lock.unlock() }

    func loudness(_ to: Int) -> Float {
        let n = rate / 8
        let begin = to - n - base
        if begin < 0 { return 0 }
        var s: Float = 0
        for i in begin..<(to - base) { s += samples[i] * samples[i] }
        return (s / Float(n)).squareRoot()
    }

    func tick() {
        lock.lock()
        let now = Date()
        let end = total
        let rms = loudness(end)
        level = max(level * 0.9, rms)
        if rms < floorLevel { floorLevel = floorLevel * 0.8 + rms * 0.2 } else if !inUtterance { floorLevel = min(floorLevel * 1.003 + 0.00002, 0.03) }
        let speaking = rms > max(0.0025, floorLevel * 2.5)           // a quiet room mic gives ~0.01 for speech; a mixer feed is far louder
        if speaking { lastSpeechAt = now }
        if !inUtterance, speaking {
            inUtterance = true
            utterStart = max(base, end - Int(0.4 * Double(rate)))
            // Speech that starts soon after the last cut is probably the same thought ("...John chapter three" / "verse sixteen"),
            // so start this one a few seconds back. Whisper then sees the whole phrase.
            if prevEnd >= 0 && Double(end - prevEnd) / Double(rate) < 4.0 { utterStart = max(base, prevEnd - Int(overlapSeconds * Double(rate))) }
            lastRequestAt = Date.distantPast
        }
        var job: (start: Int, end: Int, final: Bool)? = nil
        if inUtterance {
            let silentFor = now.timeIntervalSince(lastSpeechAt)
            let length = Double(end - utterStart) / Double(rate)
            // If what we have so far ends on a number or on "chapter"/"verse", the speaker is probably mid-reference
            // ("John chapter three ... verse sixteen"), so wait longer before cutting.
            let midReference = lastText.range(of: "(\\d|chapter|verse|colon)\\W*$", options: [.regularExpression, .caseInsensitive]) != nil
            let needSilence = midReference ? max(silenceSeconds, 2.0) : silenceSeconds
            if silentFor >= needSilence || length >= maxUtterance {
                job = (utterStart, end, true)
                prevEnd = end
                inUtterance = false
                if length >= maxUtterance && silentFor < silenceSeconds {
                    inUtterance = true; utterStart = end - Int(1.5 * Double(rate))
                }
            } else if now.timeIntervalSince(lastRequestAt) >= stepSeconds && length > 0.8 && !inFlight {
                job = (utterStart, end, false)
            }
        }
        // Free old audio, but never in the same tick that hands a sentence to be transcribed: the copy of that sentence is
        // taken in submit(), and trimming first used to cut it down to the last 2 seconds.
        if job == nil {
            let keepFrom = inUtterance ? utterStart - rate : end - rate * 8
            if keepFrom - base > rate * 20 { samples.removeFirst(keepFrom - base); base = keepFrom }
        }
        lock.unlock()
        if let j = job { submit(j.start, j.end, j.final) }
    }

    func submit(_ start: Int, _ end: Int, _ final: Bool) {
        lock.lock()
        if !final && inFlight { lock.unlock(); return }
        let from = max(base, start) - base, to = min(total, end) - base
        let slice = from < to ? Array(samples[from..<to]) : []
        if !final { inFlight = true; lastRequestAt = Date() }
        lock.unlock()
        if slice.count < rate * 3 / 10 { if !final { lock.lock(); inFlight = false; lock.unlock() }; return }
        if let dir = opt("--debug-dir") {            // keep each cut of audio for troubleshooting (off by default)
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            let name = String(format: "%@/%07.2f-%@.wav", dir, Date().timeIntervalSince(t0), final ? "final" : "partial")
            try? wav(slice).write(to: URL(fileURLWithPath: name))
        }
        work.async {
            let text = self.transcribe(slice)
            if !final { self.lock.lock(); self.inFlight = false; self.lock.unlock() }
            guard let t = text else { return }
            if t != self.lastText || final { emit(final ? "final" : "partial", t, self.t0) }
            self.lastText = final ? "" : t
        }
    }

    func wav(_ s: [Float]) -> Data {
        var d = Data()
        func u32(_ v: UInt32) { var x = v.littleEndian; d.append(Data(bytes: &x, count: 4)) }
        func u16(_ v: UInt16) { var x = v.littleEndian; d.append(Data(bytes: &x, count: 2)) }
        d.append("RIFF".data(using: .ascii)!); u32(UInt32(36 + s.count * 2)); d.append("WAVEfmt ".data(using: .ascii)!)
        u32(16); u16(1); u16(1); u32(UInt32(rate)); u32(UInt32(rate * 2)); u16(2); u16(16)
        d.append("data".data(using: .ascii)!); u32(UInt32(s.count * 2))
        var pcm = [Int16](repeating: 0, count: s.count)
        for i in 0..<s.count { pcm[i] = Int16(max(-1, min(1, s[i])) * 32767) }
        pcm.withUnsafeBytes { d.append(contentsOf: $0) }
        return d
    }

    func transcribe(_ s: [Float]) -> String? {
        let boundary = "vv" + UUID().uuidString
        var body = Data()
        func field(_ n: String, _ v: String) { body.append("--\(boundary)\r\nContent-Disposition: form-data; name=\"\(n)\"\r\n\r\n\(v)\r\n".data(using: .utf8)!) }
        field("temperature", "0.0"); field("response_format", "json"); field("no_timestamps", "true")
        if !prompt.isEmpty { field("prompt", prompt) }
        body.append("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n".data(using: .utf8)!)
        body.append(wav(s)); body.append("\r\n--\(boundary)--\r\n".data(using: .utf8)!)
        var req = URLRequest(url: url.appendingPathComponent("inference")); req.httpMethod = "POST"; req.timeoutInterval = 20
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type"); req.httpBody = body
        var result: String? = nil
        let done = DispatchSemaphore(value: 0)
        URLSession.shared.dataTask(with: req) { data, _, err in
            if let data = data, let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any], let t = j["text"] as? String {
                let clean = t.replacingOccurrences(of: "\n", with: " ").trimmingCharacters(in: .whitespaces)
                let low = clean.lowercased()
                let junk = ["", "[blank_audio]", "(silence)", "[silence]", "you", "thank you.", "thanks for watching!", "[music]", "(music)"]
                result = junk.contains(low) || (low.hasPrefix("[") && low.hasSuffix("]")) || (low.hasPrefix("(") && low.hasSuffix(")")) ? nil : clean
            } else if let err = err { emit("status", "whisper error: \(err.localizedDescription)", self.t0) }
            done.signal()
        }.resume()
        _ = done.wait(timeout: .now() + 25)
        return result
    }
}

var chanPeak = [Float](repeating: 0, count: 8)
if engineName == "whisper" {
    guard let u = opt("--whisper"), let url = URL(string: u) else { emit("status", "--whisper http://host:port is required", t0); exit(8) }
    let wl = WhisperListener(url: url, prompt: opt("--prompt") ?? "", t0: t0, step: Double(opt("--step") ?? "") ?? 1.0,
                             silence: Double(opt("--silence") ?? "") ?? 0.6, maxUtter: Double(opt("--max-utterance") ?? "") ?? 14)
    guard let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false),
          let monoFmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: fmt.sampleRate, channels: 1, interleaved: false),
          let conv = AVAudioConverter(from: monoFmt, to: target) else { emit("status", "cannot convert audio", t0); exit(9) }
    // Mixer and Dante feeds are often on one channel only and can be very quiet. So: mix down ALL channels together (or only the one
    // named with --channel N), and level the volume with a slow automatic gain so the silence detector works.
    let forcedChannel = Int(opt("--channel") ?? "") ?? 0
    var slowPeak = [Float](repeating: 0, count: 16)
    var agc: Float = 1
    engine.inputNode.installTap(onBus: 0, bufferSize: 2048, format: fmt) { buf, _ in
        guard let cd = buf.floatChannelData, buf.frameLength > 0 else { return }
        let n = Int(buf.frameLength), nch = min(Int(buf.format.channelCount), 16)
        for c in 0..<nch {
            var m: Float = 0; for i in 0..<n { m = max(m, abs(cd[c][i])) }
            if c < 8 { chanPeak[c] = max(chanPeak[c], m) }
            slowPeak[c] = max(m, slowPeak[c] * 0.9995)
        }
        let loudest = slowPeak.prefix(nch).max() ?? 0
        let use: [Int] = forcedChannel > 0 ? [min(forcedChannel, nch) - 1] : Array(0..<nch)      // default: always listen to every channel (both Dante channels), mixed together
        guard let mono = AVAudioPCMBuffer(pcmFormat: monoFmt, frameCapacity: buf.frameLength), let md = mono.floatChannelData?[0] else { return }
        mono.frameLength = buf.frameLength
        let chans = use.isEmpty ? [0] : use
        for i in 0..<n { var x: Float = 0; for c in chans { x += cd[c][i] }; md[i] = x / Float(chans.count) }
        var peak: Float = 0; for i in 0..<n { peak = max(peak, abs(md[i])) }
        let used = chans.map { slowPeak[$0] }.max() ?? 0
        let wantGain = min(40, max(1, 0.25 / max(used, 0.0005)))       // aim for peaks around 0.25; never more than 40x
        agc = wantGain < agc ? wantGain : agc * 0.999 + wantGain * 0.001
        if agc != 1 { for i in 0..<n { md[i] = max(-1, min(1, md[i] * agc)) } }
        let cap = AVAudioFrameCount(Double(n) * 16000.0 / fmt.sampleRate) + 32
        guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: cap) else { return }
        var fed = false
        var err: NSError?
        conv.convert(to: out, error: &err) { _, status in
            if fed { status.pointee = .noDataNow; return nil }
            fed = true; status.pointee = .haveData; return mono
        }
        if let ch = out.floatChannelData?[0], out.frameLength > 0 { wl.append(Array(UnsafeBufferPointer(start: ch, count: Int(out.frameLength)))) }
    }
    engine.prepare()
    do { try engine.start() } catch { emit("status", "audio engine failed: \(error.localizedDescription)", t0); exit(6) }
    emit("status", String(format: "VerseVision Listen 0.9 - Whisper (%.0f Hz input, server %@)", fmt.sampleRate, u), t0)
    Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in wl.tick() }
    Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in
        let n = Int(fmt.channelCount)
        let per = (0..<min(n, 8)).map { String(format: "ch%d %.3f", $0 + 1, chanPeak[$0]) }.joined(separator: ", ")
        emit("status", String(format: "level %.3f gain x%.0f (%@)", level, agc, per), t0); chanPeak = [Float](repeating: 0, count: 8)
    }
    if let s = opt("--seconds"), let n = Double(s) { DispatchQueue.main.asyncAfter(deadline: .now() + n) { engine.stop(); exit(0) } }
    RunLoop.main.run()
    exit(0)
}

// ======================================================================= Apple engine (fallback)
guard let rec = SFSpeechRecognizer(locale: Locale(identifier: "en-US")), rec.isAvailable else { emit("status", "recognizer unavailable", t0); exit(4) }
rec.defaultTaskHint = .dictation
if let file = opt("--file") {
    let req = SFSpeechURLRecognitionRequest(url: URL(fileURLWithPath: file))
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
var request: SFSpeechAudioBufferRecognitionRequest?
var task: SFSpeechRecognitionTask?
var lastPartial = ""
var generation = 0
func startTask() {
    generation += 1
    let g = generation
    let r = SFSpeechAudioBufferRecognitionRequest()
    r.shouldReportPartialResults = true
    r.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition
    r.addsPunctuation = false
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
Timer.scheduledTimer(withTimeInterval: 50, repeats: true) { _ in request?.endAudio(); DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { startTask() } }
Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in emit("status", String(format: "level %.3f", level), t0) }
if let s = opt("--seconds"), let n = Double(s) { DispatchQueue.main.asyncAfter(deadline: .now() + n) { engine.stop(); exit(0) } }
RunLoop.main.run()
