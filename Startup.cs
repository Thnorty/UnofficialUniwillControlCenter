using System.Diagnostics;
using System.Security;
using System.Security.Principal;
using Microsoft.Win32;

namespace MonsterKeyboardRGB;

/// <summary>
/// Starts the app at logon through Task Scheduler with highest privileges. The Run key can't start an app
/// that needs administrator rights without a UAC prompt.
/// </summary>
internal static class Startup
{
    const string TaskName = "MonsterKeyboardRGB";
    const string LegacyRunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";

    public static bool IsEnabled() => RunSchtasks($"/Query /TN \"{TaskName}\"") == 0;

    public static void Enable()
    {
        string user = WindowsIdentity.GetCurrent().Name;
        string xml = $"""
            <?xml version="1.0" encoding="UTF-16"?>
            <Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
              <RegistrationInfo><Description>Keyboard RGB tray app</Description></RegistrationInfo>
              <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{SecurityElement.Escape(user)}</UserId></LogonTrigger></Triggers>
              <Principals>
                <Principal id="Author">
                  <UserId>{SecurityElement.Escape(user)}</UserId>
                  <LogonType>InteractiveToken</LogonType>
                  <RunLevel>HighestAvailable</RunLevel>
                </Principal>
              </Principals>
              <Settings>
                <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
                <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
                <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
                <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
                <Priority>4</Priority>
              </Settings>
              <Actions Context="Author">
                <Exec>
                  <Command>{SecurityElement.Escape(Environment.ProcessPath)}</Command>
                  <Arguments>{MainForm.TrayArgument}</Arguments>
                </Exec>
              </Actions>
            </Task>
            """;
        string file = Path.Combine(Path.GetTempPath(), $"{TaskName}.task.xml");
        File.WriteAllText(file, xml, System.Text.Encoding.Unicode);
        try
        {
            if (RunSchtasks($"/Create /TN \"{TaskName}\" /XML \"{file}\" /F") != 0)
                throw new InvalidOperationException("Task Scheduler refused to create the startup task.");
        }
        finally
        {
            File.Delete(file);
        }
        RemoveLegacyRunEntry();
    }

    public static void Disable()
    {
        if (IsEnabled() && RunSchtasks($"/Delete /TN \"{TaskName}\" /F") != 0)
            throw new InvalidOperationException("Task Scheduler refused to delete the startup task.");
        RemoveLegacyRunEntry();
    }

    /// <summary>Earlier versions started from the Run key, which can't launch an elevated app.</summary>
    public static void RemoveLegacyRunEntry()
    {
        using var key = Registry.CurrentUser.OpenSubKey(LegacyRunKey, writable: true);
        key?.DeleteValue(TaskName, throwOnMissingValue: false);
    }

    static int RunSchtasks(string arguments)
    {
        using var process = Process.Start(new ProcessStartInfo("schtasks.exe", arguments)
        {
            CreateNoWindow = true,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        })!;
        process.StandardOutput.ReadToEnd();
        process.StandardError.ReadToEnd();
        process.WaitForExit();
        return process.ExitCode;
    }
}
