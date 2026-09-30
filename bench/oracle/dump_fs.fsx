// Dumps marksman's own CST elements for every `*.md` file in a directory.
//
// This is the ground truth for the hand-written parser: it runs the original
// Markdig-based `Marksman.Parser.parse` and prints each element with
// `Element.fmt`, the same formatter the F# snapshot tests use.
//
// Usage: dotnet fsi dump_fs.fsx <case-dir>

#r "/mnt/data/repos/marksman/Marksman/bin/Release/net9.0/LanguageServerProtocol.dll"
#r "/mnt/data/repos/marksman/Marksman/bin/Release/net9.0/marksman.dll"
#r "/mnt/data/repos/marksman/Marksman/bin/Release/net9.0/Markdig.dll"

open System
open System.IO

open Marksman

let splitLines (s: string) = s.Split([| "\r\n"; "\n"; "\r" |], StringSplitOptions.None)

match fsi.CommandLineArgs |> Seq.tryItem 1 with
| None -> eprintfn "usage: dotnet fsi dump_fs.fsx <case-dir>"
| Some dir ->
    for path in Directory.GetFiles(dir, "*.md") |> Array.sort do
        printfn "===== %s" (Path.GetFileName path)
        let content = File.ReadAllText(path)

        // `Structure` is both a type and a module here, so the type's `Cst`
        // member is used rather than `Structure.concreteElements`.
        let elements = (Parser.parse Config.ParserSettings.Default (Text.mkText content)).Cst.elements

        for el in elements do
            for line in splitLines (Cst.Element.fmt el) do
                printfn "%s" line
