// BCUninstaller v6.1 bridge for MonsterDeleter.
// This wrapper intentionally exposes only shortcut resolution and execution
// of an application's original uninstaller. It never enables BCU junk cleanup
// or force-removal functionality.

using Klocman.Tools;
using Klocman.IO;
using UninstallTools;
using UninstallTools.Factory;
using UninstallTools.Uninstaller;

namespace MonsterDeleter.BcuBridge;

internal static class Program
{
    private const int MaxShortcutResolutionHops = 4;

    private static int Main(string[] args)
    {
        try
        {
            return args.FirstOrDefault()?.ToLowerInvariant() switch
            {
                "index" when args.Length == 2 => BuildIndex(args[1]),
                "resolve" when args.Length is 2 or 3 => Resolve(args[1], args.ElementAtOrDefault(2)),
                "uninstall" when args.Length is 3 or 4 => Uninstall(args[1], args[2], args.Skip(3).Any(x => x == "--quiet")),
                "uninstall-batch" when args.Length >= 3 => UninstallBatch(args.Skip(1).ToArray()),
                _ => InvalidArguments(),
            };
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error.Message);
            return 13;
        }
    }

    private static int Resolve(string shortcut, string? indexPath)
    {
        // A resolvable executable shortcut is distinct from a plain file
        // shortcut: the overlay can offer a safe "delete shortcut only"
        // choice even when no official uninstaller can be found.
        if (ResolveApplicationTarget(shortcut) is null)
            return 2;

        var indexedCandidates = FindIndexedCandidates(shortcut, indexPath);
        var candidates = FindCandidates(shortcut);
        if (candidates.Count != 1)
        {
            Console.WriteLine("EXECUTABLE");
            return 0;
        }

        var entry = candidates[0];
        // The index is only a performance hint. It must never make a stale
        // entry look uninstallable after the program has been removed.
        if (indexedCandidates?.Count == 1 &&
            !string.Equals(indexedCandidates[0].Id, entry.GetCacheId(), StringComparison.OrdinalIgnoreCase))
        {
            Console.WriteLine("EXECUTABLE");
            return 0;
        }

        // The tab-separated protocol keeps the Rust side dependency-free.
        Console.WriteLine($"MATCH\t{Encode(entry.GetCacheId())}\t{Encode(entry.DisplayName)}");
        return 0;
    }

    private static int BuildIndex(string indexPath)
    {
        var destination = Path.GetFullPath(indexPath);
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);

        // Build one complete index of the Windows installed-program inventory:
        // MSI plus both registry views. This is the authoritative data the
        // app can safely launch an uninstaller from. BCU's optional Windows
        // Features and orphan-disk passes are intentionally excluded: they
        // can block an installer on WMI/Windows Update and do not provide a
        // safe software-uninstaller command for this workflow.
        var entries = new RegistryFactory(MsiTools.MsiEnumProducts()).GetUninstallerEntries(_ => { })
            .Where(entry => entry.UninstallPossible && !entry.SystemComponent)
            .Where(entry => entry.UninstallerKind != UninstallerType.SimpleDelete)
            .Select(entry => new IndexedEntry(
                entry.GetCacheId(),
                entry.DisplayName,
                entry.InstallLocation,
                entry.DisplayIcon))
            .Where(entry => !string.IsNullOrWhiteSpace(entry.Id) && !string.IsNullOrWhiteSpace(entry.DisplayName))
            .GroupBy(entry => entry.Id, StringComparer.OrdinalIgnoreCase)
            .Select(group => group.First())
            .ToList();

        var temporary = destination + ".new";
        File.WriteAllLines(temporary, entries.Select(entry => string.Join('\t',
            Encode(entry.Id),
            Encode(entry.DisplayName),
            Encode(entry.InstallLocation),
            Encode(entry.DisplayIcon))));
        File.Move(temporary, destination, true);
        return 0;
    }

    private static int Uninstall(string shortcut, string encodedId, bool quiet)
    {
        var expectedId = Decode(encodedId);
        var candidates = FindCandidates(shortcut)
            .Where(entry => string.Equals(entry.GetCacheId(), expectedId, StringComparison.Ordinal))
            .ToList();
        if (candidates.Count != 1)
            return 2;

        // `quiet` only selects a quiet command supplied or derived by BCU. The
        // wrapper never passes /U, /J, force-removal, or any cleanup operation.
        candidates[0].RunUninstaller(quiet, simulate: false);
        return 0;
    }

    private static int UninstallBatch(string[] arguments)
    {
        var quiet = arguments.Contains("--quiet", StringComparer.Ordinal);
        var pairs = arguments.Where(argument => argument != "--quiet").ToArray();
        if (pairs.Length == 0 || pairs.Length % 2 != 0) return 1;

        var requests = new List<(string Target, string Id)>();
        for (var i = 0; i < pairs.Length; i += 2)
        {
            var target = ResolveApplicationTarget(pairs[i]);
            if (target is null) return 2;
            requests.Add((target, Decode(pairs[i + 1])));
        }

        ConfigureBcu();
        var entries = new RegistryFactory(MsiTools.MsiEnumProducts()).GetUninstallerEntries(_ => { })
            .Where(entry => entry.UninstallPossible && !entry.SystemComponent)
            .Where(entry => entry.UninstallerKind != UninstallerType.SimpleDelete)
            .ToList();
        var selected = new List<ApplicationUninstallerEntry>();
        foreach (var request in requests)
        {
            var directory = Path.GetDirectoryName(request.Target);
            if (string.IsNullOrWhiteSpace(directory)) return 2;
            var candidates = entries.Where(entry =>
                    string.Equals(entry.GetCacheId(), request.Id, StringComparison.Ordinal) &&
                    MatchesShortcutTarget(entry, request.Target, directory))
                .ToList();
            if (candidates.Count != 1) return 2;
            selected.Add(candidates[0]);
        }

        // Different shortcuts can point to the same app.  Run its official
        // uninstaller once; no BCU force-removal or residual cleanup is used.
        foreach (var entry in selected
                     .GroupBy(entry => entry.GetCacheId(), StringComparer.OrdinalIgnoreCase)
                     .Select(group => group.First()))
            entry.RunUninstaller(quiet, simulate: false);
        return 0;
    }

    private static List<ApplicationUninstallerEntry> FindCandidates(string shortcut)
    {
        var target = ResolveApplicationTarget(shortcut);
        if (target is null) return [];

        var targetDirectory = Path.GetDirectoryName(target);
        if (string.IsNullOrWhiteSpace(targetDirectory))
            return [];

        ConfigureBcu();
        // The full BCU factory also surveys optional ecosystems (Windows
        // Features, Store, Steam, etc.) and can take minutes or require extra
        // privileges. For a shortcut-to-desktop-app action, use BCU's mature
        // MSI + installed-program registry pipeline only; it is deterministic
        // and finishes quickly enough for an interactive overlay.
        var entries = new RegistryFactory(MsiTools.MsiEnumProducts()).GetUninstallerEntries(_ => { });
        return entries
            .Where(entry => entry.UninstallPossible && !entry.SystemComponent)
            .Where(entry => entry.UninstallerKind != UninstallerType.SimpleDelete)
            .Where(entry => MatchesShortcutTarget(entry, target, targetDirectory))
            .GroupBy(entry => entry.GetCacheId(), StringComparer.OrdinalIgnoreCase)
            .Select(group => group.First())
            .ToList();
    }

    private static List<IndexedEntry>? FindIndexedCandidates(string shortcut, string? indexPath)
    {
        if (string.IsNullOrWhiteSpace(indexPath) || !File.Exists(indexPath)) return null;
        var target = ResolveApplicationTarget(shortcut);
        if (target is null) return null;
        var targetDirectory = Path.GetDirectoryName(target);
        if (string.IsNullOrWhiteSpace(targetDirectory)) return null;

        try
        {
            return File.ReadLines(indexPath)
                .Select(ParseIndexedEntry)
                .Where(entry => entry is not null)
                .Cast<IndexedEntry>()
                .Where(entry => MatchesShortcutTarget(entry, target, targetDirectory))
                .GroupBy(entry => entry.Id, StringComparer.OrdinalIgnoreCase)
                .Select(group => group.First())
                .ToList();
        }
        catch
        {
            return null;
        }
    }

    private static IndexedEntry? ParseIndexedEntry(string line)
    {
        var parts = line.Split('\t');
        if (parts.Length != 4) return null;
        try
        {
            return new IndexedEntry(Decode(parts[0]), Decode(parts[1]), Decode(parts[2]), Decode(parts[3]));
        }
        catch
        {
            return null;
        }
    }

    private static string? ResolveApplicationTarget(string path)
    {
        try
        {
            var current = Path.GetFullPath(path);
            if (current.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
                return File.Exists(current) ? current : null;
            if (!current.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase)) return null;

            var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            for (var hop = 0; hop < MaxShortcutResolutionHops; hop++)
            {
                // Reject a missing link and any loop such as A.lnk -> B.lnk -> A.lnk.
                if (!File.Exists(current) || !visited.Add(current)) return null;

                var target = WindowsTools.ResolveShortcut(current);
                if (string.IsNullOrWhiteSpace(target)) return null;
                current = Path.GetFullPath(target);

                if (current.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase)) continue;
                return File.Exists(current) && Path.GetExtension(current).Equals(".exe", StringComparison.OrdinalIgnoreCase)
                    ? current
                    : null;
            }

            // Do not follow arbitrarily deep chains; they are unlikely to be
            // intentional application shortcuts and are unsafe to classify.
            return null;
        }
        catch
        {
            return null;
        }
    }

    private static void ConfigureBcu()
    {
        UninstallToolsGlobalConfig.QuietAutomatization = false;
        UninstallToolsGlobalConfig.UseQuietUninstallDaemon = false;
    }

    private static bool MatchesShortcutTarget(ApplicationUninstallerEntry entry, string target, string targetDirectory) =>
        MatchesShortcutTarget(entry.DisplayIcon, entry.InstallLocation, entry.IsInstallLocationValid(), target, targetDirectory);

    private static bool MatchesShortcutTarget(IndexedEntry entry, string target, string targetDirectory) =>
        MatchesShortcutTarget(entry.DisplayIcon, entry.InstallLocation, !string.IsNullOrWhiteSpace(entry.InstallLocation), target, targetDirectory);

    private static bool MatchesShortcutTarget(string? displayIcon, string? installLocation, bool hasValidInstallLocation, string target, string targetDirectory)
    {
        // An exact DisplayIcon executable is the strongest association. Many
        // installers omit InstallLocation, especially per-user applications.
        var displayIconPath = ExtractExecutablePath(displayIcon);
        if (!string.IsNullOrWhiteSpace(displayIconPath) && PathTools.PathsEqual(target, displayIconPath))
            return true;

        // A shortcut may point into a `bin` subfolder while the registered
        // install directory is the product root. Do not match a generic
        // Program Files root: BCU's validity check excludes those broad roots.
        return hasValidInstallLocation
               && !string.IsNullOrWhiteSpace(installLocation)
               && (PathTools.PathsEqual(targetDirectory, installLocation)
                   || targetDirectory.StartsWith(
                       installLocation.TrimEnd('\\', '/') + Path.DirectorySeparatorChar,
                       StringComparison.OrdinalIgnoreCase));
    }

    private static string? ExtractExecutablePath(string? displayIcon)
    {
        if (string.IsNullOrWhiteSpace(displayIcon)) return null;
        var value = displayIcon.Trim();
        if (value.StartsWith('"'))
        {
            var end = value.IndexOf('"', 1);
            return end > 1 ? value[1..end] : null;
        }
        var comma = value.IndexOf(',');
        return (comma >= 0 ? value[..comma] : value).Trim();
    }

    private static string Encode(string? value) => Convert.ToBase64String(System.Text.Encoding.UTF8.GetBytes(value ?? string.Empty));
    private static string Decode(string value) => System.Text.Encoding.UTF8.GetString(Convert.FromBase64String(value));

    private sealed record IndexedEntry(string Id, string DisplayName, string? InstallLocation, string? DisplayIcon);

    private static int InvalidArguments()
    {
        Console.Error.WriteLine("Usage: bcu-bridge index <cache-path> | resolve <target> [cache-path] | uninstall <target> <candidate-id> [--quiet] | uninstall-batch [--quiet] <target> <candidate-id> [...]");
        return 1;
    }
}
