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
    var speakingRun = 0
    var announcedOn = false
    var announcedAt = Date.distantPast
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
    let partialSeconds = Double(opt("--partial-seconds") ?? "") ?? 8.0     // live updates look at the last 8 s only; the final cut keeps the whole sentence

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
        // Tell the other end the instant speech begins (before any word is transcribed). Needs 0.15 s of sound so a click is ignored.
        if speaking { speakingRun += 1 } else { speakingRun = 0 }
        if !announcedOn, speakingRun >= 2 { announcedOn = true; announcedAt = now; emit("status", "speech start", t0) }
        if announcedOn, !speaking, now.timeIntervalSince(lastSpeechAt) >= 0.3 { announcedOn = false; emit("status", "speech end", t0) }
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
            } else if now.timeIntervalSince(lastRequestAt) >= stepSeconds && length > 0.6 && !inFlight {
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
        let window = final ? Int.max : Int(partialSeconds * Double(rate))
        let from = max(max(base, start), end - window) - base, to = min(total, end) - base
        let slice = from < to ? Array(samples[from..<to]) : []
        if !final { inFlight = true; lastRequestAt = Date() }
        lock.unlock()
        if slice.count < rate * 3 / 10 { if !final { lock.lock(); inFlight = false; lock.unlock() }; return }
        // Whisper invents words (often the end of its own prompt) when it is given near-silence, so audio with no real speech in it
        // is never sent. "Real speech" = the loudest tenth of a second is clearly above the room noise.
        var loudest: Float = 0
        let hop = rate / 10
        var i0 = 0
        while i0 < slice.count { let e = min(slice.count, i0 + hop); var ss: Float = 0; for k in i0..<e { ss += slice[k] * slice[k] }; loudest = max(loudest, (ss / Float(e - i0)).squareRoot()); i0 = e }
        if loudest < max(0.012, floorLevel * 3) { if !final { lock.lock(); inFlight = false; lock.unlock() }; return }
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
        var req = URLRequest(url: url.appendingPathComponent("inference")); req.httpMethod = "POST"; req.timeoutInterval = 7
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
var mixPeak: Float = 0, mixRms: Float = 0, curGain: Float = 1
func dB(_ x: Float) -> String { x < 0.00001 ? "-inf" : String(format: "%.0f", 20 * log10(Double(x))) }
if engineName == "whisper" {
    guard let u = opt("--whisper"), let url = URL(string: u) else { emit("status", "--whisper http://host:port is required", t0); exit(8) }
    let wl = WhisperListener(url: url, prompt: opt("--prompt") ?? "", t0: t0, step: Double(opt("--step") ?? "") ?? 0.5,
                             silence: Double(opt("--silence") ?? "") ?? 0.6, maxUtter: Double(opt("--max-utterance") ?? "") ?? 9)
    guard let target = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false),
          let monoFmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: fmt.sampleRate, channels: 1, interleaved: false),
          let conv = AVAudioConverter(from: monoFmt, to: target) else { emit("status", "cannot convert audio", t0); exit(9) }
    // Mixer and Dante feeds are often on one channel only and can be very quiet. So: mix down ALL channels together (or only the one
    // named with --channel N), and level the volume with a slow automatic gain so the silence detector works.
    let forcedChannel = Int(opt("--channel") ?? "") ?? 0
    let targetRMS: Float = 0.07                // loud-ish speech: about -23 dBFS RMS. Whisper does best with a healthy level
    let maxGain: Float = 30                    // never boost more than +29.5 dB
    var speechEnv: Float = 0                   // how loud the speaker has been lately (RMS of the loud buffers, decaying slowly)
    var peakEnv: Float = 0                     // recent peaks, so a boost never clips
    var noiseFloor: Float = 0.0005
    var agc: Float = 1
    engine.inputNode.installTap(onBus: 0, bufferSize: 2048, format: fmt) { buf, _ in
        guard let cd = buf.floatChannelData, buf.frameLength > 0 else { return }
        let n = Int(buf.frameLength), nch = min(Int(buf.format.channelCount), 16)
        for c in 0..<min(nch, 8) { var m: Float = 0; for i in 0..<n { m = max(m, abs(cd[c][i])) }; chanPeak[c] = max(chanPeak[c], m) }
        guard let mono = AVAudioPCMBuffer(pcmFormat: monoFmt, frameCapacity: buf.frameLength), let md = mono.floatChannelData?[0] else { return }
        mono.frameLength = buf.frameLength
        let chans: [Int] = forcedChannel > 0 ? [min(forcedChannel, nch) - 1] : Array(0..<nch)      // default: every channel (both Dante channels)
        // Mix the channels that carry sound. A dead channel (silence) must not halve the signal of the live one, so the mix is
        // the sum divided by the number of channels that are actually active (at least one).
        var act: [Int] = []
        for c in chans { var m: Float = 0; for i in 0..<n { m = max(m, abs(cd[c][i])) }; if m > 0.0002 { act.append(c) } }
        let use = act.isEmpty ? chans : act
        var sumSq: Float = 0, peak: Float = 0
        for i in 0..<n { var x: Float = 0; for c in use { x += cd[c][i] }; x /= Float(use.count); md[i] = x; sumSq += x * x; peak = max(peak, abs(x)) }
        let rms = (sumSq / Float(n)).squareRoot()
        mixPeak = max(mixPeak, peak); mixRms = max(mixRms, rms)
        // Level control. The 'speech' buffers (clearly above the noise) set the target; silence leaves the gain alone, so the
        // room noise is never pumped up.
        if rms < noiseFloor * 2 { noiseFloor = noiseFloor * 0.98 + rms * 0.02 } else if rms < noiseFloor * 3 { noiseFloor = min(noiseFloor * 1.002, 0.01) }
        let speechy = rms > max(0.0015, noiseFloor * 4)
        peakEnv = max(peak, peakEnv * 0.9977)                       // about 12 s half-life
        if speechy { speechEnv = max(rms, speechEnv * 0.9977) }
        if speechEnv > 0 {
            let want = min(maxGain, max(1, targetRMS / speechEnv), 0.9 / max(peakEnv, 0.001))
            if want < agc { agc = want }                            // too loud: come down at once
            else if speechy { agc += (want - agc) * 0.01 }          // quieter speaker: come up over a few seconds, only while talking
        }
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
        curGain = agc
    }
    engine.prepare()
    do { try engine.start() } catch { emit("status", "audio engine failed: \(error.localizedDescription)", t0); exit(6) }
    emit("status", String(format: "VerseVision Listen 0.9 - Whisper (%.0f Hz input, server %@)", fmt.sampleRate, u), t0)
    Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in wl.tick() }
    Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in
        let n = Int(fmt.channelCount)
        let per = (0..<min(n, 8)).map { String(format: "ch%d %.3f", $0 + 1, chanPeak[$0]) }.joined(separator: ", ")
        emit("status", String(format: "level %.3f gain x%.1f (%+.0f dB); input peak %@ dBFS, rms %@ dBFS; after gain peak %@ dBFS (%@)", level, curGain, 20 * log10(Double(max(curGain, 0.001))), dB(mixPeak), dB(mixRms), dB(min(1, mixPeak * curGain)), per), t0)
        chanPeak = [Float](repeating: 0, count: 8); mixPeak = 0; mixRms = 0
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
