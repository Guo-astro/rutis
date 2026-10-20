// The text in an image, one recognized line per output line (Vision): what
// tests/e2e/tests/ios_app.rs reads off a simulator's screenshot.
//
//   xcrun swift tests/e2e/ios/text.swift <image.png>
import Foundation
import Vision

guard CommandLine.arguments.count == 2,
      let image = CGImageSourceCreateWithURL(URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL, nil)
      .flatMap({ CGImageSourceCreateImageAtIndex($0, 0, nil) })
else {
    FileHandle.standardError.write(Data("usage: text.swift <image>\n".utf8))
    exit(2)
}
let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
try VNImageRequestHandler(cgImage: image).perform([request])
for observation in request.results ?? [] {
    if let line = observation.topCandidates(1).first?.string { print(line) }
}
