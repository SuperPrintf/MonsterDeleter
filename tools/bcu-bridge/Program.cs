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
    private static int Main(string[] args)
    {
        try
        {
            return args.FirstOrDefault()?.ToLowerInvariant() switch
            {
                "index" when args.Length == 2 => BuildIndex(args[1]),
                "resolve" when args.Length is 2 or 3 => Resolve(args[1], args.ElementAtOrDefault(2)),
                "uninstall" when args.Length is 3 or 4 => Uninstall(args[1], args[2], args.Skip(3).Any(x => x == "--quiet")),
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
        var indexedCandidates = FindIndexedCandidates(shortcut, indexPath);
        if (indexedCandidates?.Count == 1)
        {
            var indexed = indexedCandidates[0];
            Console.WriteLine($"MATCH\t{Encode(indexed.Id)}\t{Encode(indexed.DisplayName)}");
            return 0;
        }

        var candidates = FindCandidates(shortcut);
        if (candidates.Count != 1)
            return 2;

        var entry = candidates[0];
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

    private static List<ApplicationUninstallerEntry> FindCandidates(string shortcut)
    {
        var target = shortcut.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase)
            ? WindowsTools.ResolveShortcut(shortcut)
            : shortcut;
        if (string.IsNullOrWhiteSpace(target) || !File.Exists(target))
            return [];

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
        var target = ResolveShortcutTarget(shortcut);
        if (string.IsNullOrWhiteSpace(target) || !File.Exists(target)) return null;
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

    private static string? ResolveShortcutTarget(string shortcut)
    {
        var target = shortcut.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase)
            ? WindowsTools.ResolveShortcut(shortcut)
            : shortcut;
        return string.IsNullOrWhiteSpace(target) ? null : target;
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
        Console.Error.WriteLine("Usage: bcu-bridge index <cache-path> | resolve <shortcut> [cache-path] | uninstall <shortcut> <candidate-id> [--quiet]");
        return 1;
    }
}
