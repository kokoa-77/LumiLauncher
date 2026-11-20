using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Markup;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Imaging;
using Rectangle = System.Windows.Shapes.Rectangle;
using Forms = System.Windows.Forms;

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        try
        {
            if (args.Length != 6 || (args[0] != "install" && args[0] != "remove" && args[0] != "update"))
                return 2;
            var app = new Application();
            var setup = new SetupWindow(args);
            app.Run(setup.View);
            return setup.Result;
        }
        catch (Exception error)
        {
            MessageBox.Show(error.Message, "Lumi", MessageBoxButton.OK, MessageBoxImage.Error);
            return 2;
        }
    }
}

internal sealed class SetupWindow
{
    private readonly bool zh = CultureInfo.CurrentUICulture.TwoLetterISOLanguageName == "zh";
    private readonly bool uninstall;
    private readonly bool updating;
    private readonly string engine;
    private readonly string install;
    private readonly string data;
    private readonly string binary;
    private readonly bool installed;
    private readonly string defaultGamePath;
    private bool busy;
    private bool succeeded;
    private int page;
    private bool transitioning;
    private int lastCheckpoint;
    private Rectangle progressGleam;
    private string lastStage;
    private string selectedInstall;
    private string selectedGames;

    internal Window View { get; private set; }
    internal int Result { get; private set; }

    internal SetupWindow(string[] args)
    {
        uninstall = args[0] == "remove";
        updating = args[0] == "update";
        engine = Path.GetFullPath(args[1]);
        install = Path.GetFullPath(args[2]);
        data = Path.GetFullPath(args[3]);
        binary = Path.GetFileName(args[4]);
        if (String.IsNullOrEmpty(binary) || !File.Exists(engine) ||
            (uninstall && !String.Equals(engine, Path.Combine(install, "uninstall.exe"), StringComparison.OrdinalIgnoreCase)))
            throw new InvalidOperationException(T("Lumi 安装程序路径无效。", "The Lumi installer path is invalid."));
        installed = File.Exists(Path.Combine(install, binary));

        using (var layout = Resource("SetupWindow.xaml"))
            View = (Window)XamlReader.Load(layout);
        View.Title = updating ? T("更新 Lumi", "Update Lumi") :
            uninstall ? T("卸载 Lumi", "Uninstall Lumi") : T("安装 Lumi", "Install Lumi");
        var brand = ImageFrom("lumi.ico");
        View.Icon = brand;
        Find<Image>("BrandIcon").Source = brand;
        Find<Image>("NightScene").Source = ImageFrom("setup-night.jpg");
        var area = SystemParameters.WorkArea;
        var scale = Math.Min(1.0, Math.Min((area.Width - 20) / View.Width, (area.Height - 20) / View.Height));
        if (scale > 0 && scale < 1)
        {
            ((FrameworkElement)View.Content).LayoutTransform = new ScaleTransform(scale, scale);
            View.Width *= scale;
            View.Height *= scale;
        }

        Find<Button>("CloseButton").Click += delegate { View.Close(); };
        Find<Button>("MinimizeButton").Click += delegate { View.WindowState = WindowState.Minimized; };
        Find<Button>("CloseButton").ToolTip = T("关闭", "Close");
        Find<Button>("MinimizeButton").ToolTip = T("最小化", "Minimize");
        System.Windows.Automation.AutomationProperties.SetName(Find<Button>("CloseButton"), T("关闭", "Close"));
        System.Windows.Automation.AutomationProperties.SetName(Find<Button>("MinimizeButton"), T("最小化", "Minimize"));
        Find<Border>("TitleBar").MouseLeftButtonDown += delegate
        {
            if (!Find<Button>("CloseButton").IsMouseOver && !Find<Button>("MinimizeButton").IsMouseOver &&
                Mouse.LeftButton == MouseButtonState.Pressed)
                View.DragMove();
        };
        View.Closing += delegate(object sender, System.ComponentModel.CancelEventArgs e) { e.Cancel = busy || transitioning; };
        View.KeyDown += delegate(object sender, KeyEventArgs e) { if (e.Key == Key.Escape) View.Close(); };
        Find<Button>("NextButton").Click += async delegate { await Next(); };
        Find<Button>("BackButton").Click += async delegate
        {
            if (busy || transitioning) return;
            if (succeeded) await CloseAfterMotion();
            else if (uninstall || updating) View.Close();
            else if (page == 2) await TransitionTo(ShowOptions, true);
            else await SwitchWelcome(true);
        };
        Find<Label>("ProgramLabel").Content = T("程序安装位置", "Launcher location");
        Find<Label>("GameLabel").Content = T("游戏文件位置", "Game files");
        Text("RemovalLabel", T("程序安装位置", "Launcher location"));
        Text("RemovalHint", T("实例、存档、截图、资源和 Java 会保留。", "Your instances, worlds, screenshots, resources and Java will remain."));
        Find<CheckBox>("DesktopChoice").Content = T("创建桌面快捷方式", "Create a desktop shortcut");
        Find<CheckBox>("PackChoice").Content = T("关联 .mrpack 整合包", "Register Lumi for .mrpack modpacks");
        Find<CheckBox>("LaunchChoice").Content = T("完成后打开 Lumi", "Open Lumi when finished");
        Find<CheckBox>("CleanChoice").Content = T("同时清除 Lumi 设置", "Also remove Lumi settings");

        Find<TextBox>("RemovalPath").Text = install;
        Find<TextBox>("ProgramPath").Text = install;
        Find<TextBox>("ProgramPath").IsReadOnly = installed;
        SetupBrowse("ProgramBrowse", "ProgramPath", !installed);
        SetupBrowse("GameBrowse", "GamePath", true);
        defaultGamePath = updating ? "" : PickGamePath(args[5]);
        Find<TextBox>("GamePath").Text = defaultGamePath;

        if (updating)
        {
            page = 1;
            View.Loaded += async delegate { await Next(); };
        }
        else
        {
            if (uninstall) ShowRemoval();
            else
            {
                // Prepare the folder layout before the window's first render.
                ShowOptions();
                ShowWelcome();
            }
            View.Loaded += delegate { EnterPage(1); };
        }
    }

    private string T(string chinese, string english) { return zh ? chinese : english; }
    private TControl Find<TControl>(string name) where TControl : FrameworkElement
    {
        return (TControl)View.FindName(name);
    }
    private void Text(string name, string value) { Find<TextBlock>(name).Text = value; }
    private void Visible(string name, bool value)
    {
        Find<FrameworkElement>(name).Visibility = value ? Visibility.Visible : Visibility.Collapsed;
    }

    private static Stream Resource(string name) { return Assembly.GetExecutingAssembly().GetManifestResourceStream(name); }
    private static BitmapSource ImageFrom(string name)
    {
        using (var stream = Resource(name))
        {
            var decoder = BitmapDecoder.Create(stream, BitmapCreateOptions.PreservePixelFormat, BitmapCacheOption.OnLoad);
            var image = decoder.Frames[0];
            image.Freeze();
            return image;
        }
    }

    private void SetupBrowse(string buttonName, string fieldName, bool enabled)
    {
        var button = Find<Button>(buttonName);
        Text(buttonName + "Text", T("浏览", "Browse"));
        Visible(buttonName, enabled);
        button.Click += delegate
        {
            var field = Find<TextBox>(fieldName);
            using (var picker = new Forms.FolderBrowserDialog())
            {
                picker.Description = fieldName == "GamePath" ? T("选择游戏文件位置", "Choose game files location") :
                    T("选择程序安装位置", "Choose launcher location");
                picker.SelectedPath = field.Text;
                picker.ShowNewFolderButton = true;
                if (picker.ShowDialog() == Forms.DialogResult.OK) field.Text = picker.SelectedPath;
            }
        };
    }

    private string PickGamePath(string product)
    {
        var standard = Path.Combine(data, "minecraft");
        var settings = Path.Combine(data, "settings.json");
        Text("DiskNote", T("版本、模组、存档和 Java 存放在这里。", "Versions, mods, worlds and Java live here."));
        if (File.Exists(settings) || Directory.Exists(standard))
        {
            Text("DiskNote", T("已保留当前游戏目录。切换位置不会移动原文件。", "Your current game location is kept. Choosing another won't move existing files."));
            try
            {
                var root = new JavaScriptSerializer().DeserializeObject(File.ReadAllText(settings)) as IDictionary<string, object>;
                object game;
                object value;
                if (root != null && root.TryGetValue("game", out game) && game is IDictionary<string, object> &&
                    ((IDictionary<string, object>)game).TryGetValue("directory", out value) && value is string)
                    return Path.GetFullPath((string)value);
            }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
            catch (ArgumentException) { }
            catch (InvalidOperationException)
            {
                // A malformed or missing settings file leaves the previously used default in place.
            }
            return standard;
        }
        try
        {
            var c = new DriveInfo("C");
            var space = c.AvailableFreeSpace;
            if (space >= 20L * 1024 * 1024 * 1024) return standard;
            DriveInfo best = null;
            foreach (var candidate in DriveInfo.GetDrives())
            {
                if (candidate.DriveType != DriveType.Fixed || !candidate.IsReady ||
                    String.Equals(candidate.Name, c.Name, StringComparison.OrdinalIgnoreCase)) continue;
                try
                {
                    if (candidate.AvailableFreeSpace <= space) continue;
                    best = candidate;
                    space = candidate.AvailableFreeSpace;
                }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
            if (best != null)
            {
                var driveLetter = best.Name.TrimEnd('\\', ':');
                Text("DiskNote", T("C 盘可用空间不足 20 GB，已为游戏选择 " + driveLetter + " 盘。",
                    "Drive C has less than 20 GB free. Games will be stored on drive " + driveLetter + "."));
                return Path.Combine(best.Name, product + " Games");
            }
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
        return standard;
    }

    private void Show(string section, string label, string title, string lead, string footnote, string next, string back)
    {
        bool welcome = page == 0 && !uninstall;
        Find<StackPanel>("WelcomeHero").Visibility = welcome ? Visibility.Visible : Visibility.Hidden;
        Find<StackPanel>("PageBody").Visibility = welcome ? Visibility.Hidden : Visibility.Visible;
        if (welcome)
        {
            Find<TextBlock>("WelcomeHeading").FontSize = zh ? 82 : 74;
            Text("WelcomeHeading", title);
            Text("WelcomeLead", lead);
        }
        else
        {
            foreach (var id in new[] { "Options", "Progress", "Removal", "Summary" }) Visible(id, id == section);
            Visible("Details", section.Length > 0);
            Find<ScrollViewer>("Details").CacheMode = section == "Options" ? (BitmapCache)View.FindResource("FolderCache") : null;
            Text("Heading", title);
            Text("Lead", lead);
        }
        Text("PageLabel", label);
        Text("Footnote", footnote);
        var forward = Find<Button>("NextButton");
        forward.Style = (Style)View.FindResource(uninstall && section == "Removal" ? "DangerPrimary" : "Primary");
        Text("NextText", next);
        System.Windows.Automation.AutomationProperties.SetName(forward, next);
        Visible("NextButton", next.Length > 0);
        forward.IsEnabled = !busy;
        Find<Button>("CloseButton").IsEnabled = !busy;
        var previous = Find<Button>("BackButton");
        previous.Content = back;
        previous.Visibility = back.Length > 0 ? Visibility.Visible : Visibility.Hidden;
        if (!busy && next.Length > 0) forward.Focus();
    }

    // Use Lumi's easing curves on the live controls so the night scene never has to move.
    private static DoubleAnimationUsingKeyFrames Smooth(double from, double to, int milliseconds, bool entering,
        int delay = 0)
    {
        var animation = new DoubleAnimationUsingKeyFrames { FillBehavior = FillBehavior.Stop };
        animation.KeyFrames.Add(new DiscreteDoubleKeyFrame(from, KeyTime.FromTimeSpan(TimeSpan.Zero)));
        if (delay > 0)
            animation.KeyFrames.Add(new DiscreteDoubleKeyFrame(from, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(delay))));
        animation.KeyFrames.Add(new SplineDoubleKeyFrame(to, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(milliseconds + delay)),
            entering ? new KeySpline(.22, 1, .36, 1) : new KeySpline(.4, 0, 1, 1)));
        return animation;
    }

    private static Task FadeLayer(UIElement element, double opacity, int duration, int delay = 0)
    {
        var fade = Smooth(element.Opacity, opacity, duration, opacity > 0, delay);
        var finished = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        fade.Completed += delegate { finished.TrySetResult(true); };
        element.Opacity = opacity;
        element.BeginAnimation(UIElement.OpacityProperty, fade);
        return finished.Task;
    }

    private static Task MoveLayer(FrameworkElement element, double opacity, double distance, int duration, int delay = 0)
    {
        var position = (TranslateTransform)element.RenderTransform;
        var shift = Smooth(position.Y, distance, duration, opacity > 0, delay);
        position.Y = distance;
        position.BeginAnimation(TranslateTransform.YProperty, shift);
        return FadeLayer(element, opacity, duration, delay);
    }

    private static Task EnterLayer(FrameworkElement element, double distance, int duration, int delay = 0)
    {
        element.BeginAnimation(UIElement.OpacityProperty, null);
        var position = (TranslateTransform)element.RenderTransform;
        position.BeginAnimation(TranslateTransform.YProperty, null);
        if (!SystemParameters.ClientAreaAnimation || element.Visibility != Visibility.Visible)
        {
            element.Opacity = 1;
            position.Y = 0;
            return Task.FromResult(true);
        }
        element.Opacity = 0;
        position.Y = distance;
        return MoveLayer(element, 1, 0, duration, delay);
    }

    private void EnterPage(int direction)
    {
        if (page == 0) EnterLayer(Find<StackPanel>("WelcomeHero"), 8 * direction, 280);
        else
        {
            EnterLayer(Find<StackPanel>("Hero"), 9 * direction, 280);
            EnterLayer(Find<ScrollViewer>("Details"), 7 * direction, 255, 34);
        }
        EnterLayer(Find<DockPanel>("Actions"), 4 * direction, 235, 48);
        EnterLayer(Find<TextBlock>("PageLabel"), 0, 220);
    }

    private static double PageFade(double progress)
    {
        progress = Math.Max(0, Math.Min(1, progress));
        return progress * .35 + progress * progress * (3 - 2 * progress) * .65;
    }

    private async Task SwitchWelcome(bool toWelcome)
    {
        Action show = toWelcome ? (Action)ShowWelcome : ShowOptions;
        if (!SystemParameters.ClientAreaAnimation)
        {
            show();
            EnterPage(toWelcome ? -1 : 1);
            return;
        }

        var welcome = Find<StackPanel>("WelcomeHero");
        var options = Find<StackPanel>("PageBody");
        var outgoing = toWelcome ? options : welcome;
        var incoming = toWelcome ? welcome : options;
        var labels = new UIElement[]
        {
            Find<TextBlock>("NextText"),
            Find<Button>("BackButton"),
            Find<TextBlock>("Footnote"),
            Find<TextBlock>("PageLabel")
        };
        transitioning = true;
        bool wasHitTestVisible = outgoing.IsHitTestVisible;
        outgoing.IsHitTestVisible = false;

        // Change pages only at zero opacity. Keep the accent button and night scene
        // outside the fade, and drive the page and labels from the same render callback.
        const double fadeOut = 170;
        const double fadeIn = 220;
        const double prepare = 28;
        const double duration = fadeOut + fadeIn;
        double initialOpacity = outgoing.Opacity;
        outgoing.BeginAnimation(UIElement.OpacityProperty, null);
        foreach (var label in labels) label.BeginAnimation(UIElement.OpacityProperty, null);
        bool switched = false;
        var finished = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        var time = Stopwatch.StartNew();
        EventHandler render = delegate
        {
            double elapsed = time.Elapsed.TotalMilliseconds;
            // Let the composition channel synchronize before changing the page's opacity.
            if (elapsed < prepare) return;
            elapsed = (elapsed - prepare) * duration / (duration - prepare);
            if (elapsed >= fadeOut && !switched)
            {
                incoming.Opacity = 0;
                foreach (var label in labels) label.Opacity = 0;
                try { show(); }
                catch (Exception error)
                {
                    finished.TrySetException(error);
                    return;
                }
                switched = true;
            }

            double opacity = switched
                ? PageFade((elapsed - fadeOut) / fadeIn)
                : initialOpacity * (1 - PageFade(elapsed / fadeOut));
            if (switched) incoming.Opacity = opacity;
            else outgoing.Opacity = opacity;
            foreach (var label in labels) label.Opacity = opacity;
            if (elapsed >= duration) finished.TrySetResult(true);
        };
        CompositionTarget.Rendering += render;
        try
        {
            await finished.Task;
        }
        finally
        {
            CompositionTarget.Rendering -= render;
            incoming.Opacity = 1;
            outgoing.Opacity = 1;
            outgoing.IsHitTestVisible = wasHitTestVisible;
            foreach (var label in labels) label.Opacity = 1;
            transitioning = false;
        }
    }

    private async Task TransitionTo(Action show, bool back = false)
    {
        int direction = back ? -1 : 1;
        if (SystemParameters.ClientAreaAnimation && View.IsLoaded && Find<TextBlock>("Heading").Text.Length > 0)
        {
            transitioning = true;
            var exit = new List<Task>
            {
                MoveLayer(Find<StackPanel>("Hero"), 0, -5 * direction, 110),
                MoveLayer(Find<DockPanel>("Actions"), 0, -2 * direction, 100),
                MoveLayer(Find<TextBlock>("PageLabel"), 0, 0, 105)
            };
            var details = Find<ScrollViewer>("Details");
            if (details.Visibility == Visibility.Visible)
                exit.Add(MoveLayer(details, 0, -4 * direction, 105));
            await Task.WhenAll(exit);
        }
        show();
        EnterPage(direction);
        transitioning = false;
    }

    private Task UpdateStage(ProgressBar meter, ScaleTransform scale, string stage)
    {
        if (stage != null && stage.StartsWith("done:", StringComparison.Ordinal)) stage = "done";
        if (lastStage == stage) return Task.FromResult(true);
        string description;
        int checkpoint;
        switch (stage)
        {
            case "checking":
                checkpoint = 16;
                description = T("正在检查运行环境", "Checking the Windows runtime");
                break;
            case "install":
                checkpoint = 39;
                description = updating ? T("正在更新启动器文件", "Updating launcher files") :
                    T("正在安装启动器文件", "Installing launcher files");
                break;
            case "register":
                checkpoint = 67;
                description = T("正在配置快捷方式和文件关联", "Configuring shortcuts and file associations");
                break;
            case "scan":
                checkpoint = 85;
                description = T("正在查找此设备上的游戏", "Looking for games on this device");
                break;
            case "remove":
                checkpoint = 38;
                description = T("正在移除启动器程序", "Removing the launcher");
                break;
            case "cleanup":
                checkpoint = 82;
                description = T("正在清理快捷方式和关联", "Cleaning up shortcuts and file associations");
                break;
            case "done":
                checkpoint = 100;
                description = updating ? T("正在重新打开 Lumi", "Reopening Lumi") :
                    uninstall ? T("卸载完成", "Uninstall complete") :
                    Find<CheckBox>("LaunchChoice").IsChecked == true ? T("正在打开 Lumi", "Opening Lumi") :
                    T("正在结束安装", "Finishing setup");
                break;
            default:
                return Task.FromResult(true);
        }

        if (checkpoint <= lastCheckpoint) return Task.FromResult(true);
        lastStage = stage;
        lastCheckpoint = checkpoint;
        Text("ProgressText", description);
        EnterLayer(Find<TextBlock>("ProgressText"), 3, 195);
        double previous = scale.ScaleX;
        double target = checkpoint / 100.0;
        meter.Value = checkpoint;
        scale.ScaleX = target;
        if (!SystemParameters.ClientAreaAnimation)
        {
            scale.BeginAnimation(ScaleTransform.ScaleXProperty, null);
            return Task.FromResult(true);
        }

        var fill = Smooth(previous, target, checkpoint == 100 ? 250 : 310, true);
        var finished = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        fill.Completed += delegate { finished.TrySetResult(true); };
        scale.BeginAnimation(ScaleTransform.ScaleXProperty, fill);
        return finished.Task;
    }

    private Rectangle StartGleam(ProgressBar meter)
    {
        if (!SystemParameters.ClientAreaAnimation) return null;
        meter.ApplyTemplate();
        var gleam = meter.Template.FindName("BarGleam", meter) as Rectangle;
        if (gleam == null) return null;
        var glide = new TranslateTransform(-105, 0);
        gleam.RenderTransform = glide;
        gleam.Opacity = 1;
        var sweep = new DoubleAnimation(-105, Find<StackPanel>("PageBody").Width, TimeSpan.FromMilliseconds(1600))
        {
            RepeatBehavior = RepeatBehavior.Forever
        };
        glide.BeginAnimation(TranslateTransform.XProperty, sweep);
        return gleam;
    }

    private void StopGleam()
    {
        if (progressGleam == null) return;
        ((TranslateTransform)progressGleam.RenderTransform).BeginAnimation(TranslateTransform.XProperty, null);
        progressGleam.Opacity = 0;
        progressGleam = null;
    }

    private void ShowWelcome()
    {
        page = 0;
        Show("", T("安装 Lumi", "Install Lumi"), T("点亮你的世界", "Light up your world"),
            T("安装 Lumi，整理你的 Minecraft Java 版游戏，再回到熟悉的世界。",
              "Set up Lumi to organize Minecraft: Java Edition and return to your worlds."),
            T("安装会保留你已有的游戏和存档。", "Your existing games and worlds are kept."),
            T("开始安装", "Get started"), "");
    }

    private void ShowOptions()
    {
        page = 1;
        Show("Options", T("安装位置", "Installation folders"), T("选择安装位置", "Choose locations"),
            T("程序和游戏文件可以放在不同的磁盘。", "You can keep the launcher and game files on different drives."),
            T("游戏文件位置可在 Lumi 设置中更改。", "You can change the game location from Lumi settings."),
            T("安装", "Install"), T("上一步", "Back"));
    }

    private void ShowRemoval()
    {
        page = 1;
        Show("Removal", T("卸载 Lumi", "Uninstall Lumi"), T("卸载 Lumi", "Uninstall Lumi"),
            T("将移除 Lumi 程序和 Windows 快捷方式。", "Remove the Lumi application and its Windows shortcuts."),
            T("默认保留设置和游戏文件。", "Settings and game files are kept by default."),
            T("卸载", "Uninstall"), T("取消", "Cancel"));
    }

    private async Task CloseAfterMotion()
    {
        busy = true;
        StopGleam();
        if (SystemParameters.ClientAreaAnimation && View.WindowState != WindowState.Minimized)
        {
            var fade = Smooth(View.Opacity, 0, 160, false);
            var finished = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
            fade.Completed += delegate { finished.TrySetResult(true); };
            View.Opacity = 0;
            View.BeginAnimation(UIElement.OpacityProperty, fade);
            await finished.Task;
        }
        busy = false;
        View.Close();
    }

    private void ShowInstallComplete(int foundGames)
    {
        string title = T("Lumi 安装完成", "Lumi is installed");
        string lead = T("现在可以打开 Lumi，也可以稍后从开始菜单启动。", "Open Lumi now or launch it later from the Start menu.");
        if (foundGames > 0)
        {
            Text("SummaryTitle", String.Format(CultureInfo.CurrentCulture,
                T("已找到 {0} 个本地版本或实例", "Local game versions or instances found: {0}"), foundGames));
            Text("SummaryText", T("打开 Lumi 后可以查看并接入已有游戏。", "Open Lumi to review and add the games found on this device."));
        }
        Show(foundGames > 0 ? "Summary" : "", "", title, foundGames > 0 ? "" : lead, "",
            T("打开 Lumi", "Open Lumi"), T("完成", "Done"));
    }

    private async Task OpenInstalledLauncher()
    {
        busy = true;
        Find<Button>("NextButton").IsEnabled = false;
        Find<Button>("CloseButton").IsEnabled = false;
        Exception launchError = null;
        try
        {
            var info = new ProcessStartInfo(Path.Combine(selectedInstall, binary)) { WorkingDirectory = selectedInstall };
            await Task.Run(async () =>
            {
                using (var process = Process.Start(info))
                {
                    if (process == null) throw new InvalidOperationException(T("无法打开 Lumi。", "Lumi could not be opened."));
                    await WaitForLauncherWindow(process).ConfigureAwait(false);
                }
            });
        }
        catch (Exception error)
        {
            launchError = error;
        }
        if (launchError != null)
        {
            busy = false;
            await TransitionTo(() =>
            {
                Text("SummaryTitle", T("安装已完成", "Lumi is installed"));
                Text("SummaryText", launchError.Message);
                Show("Summary", "", T("暂时无法打开 Lumi", "Lumi could not open"), "", "",
                    T("重新打开", "Try again"), T("关闭", "Close"));
            });
            return;
        }
        await CloseAfterMotion();
    }

    private async Task WaitForLauncherWindow(Process launcher)
    {
        // The main window starts hidden and Lumi reveals it after its first frame.
        // Allow for the launcher's five-second fallback plus OS and WebView startup.
        var waiting = Stopwatch.StartNew();
        var restored = IntPtr.Zero;
        while (waiting.ElapsedMilliseconds < 8000)
        {
            launcher.Refresh();
            if (launcher.HasExited)
            {
                int code = launcher.ExitCode;
                if (code != 0)
                    throw new InvalidOperationException(T("Lumi 启动进程已退出。代码 ", "Lumi exited while opening. Code ") + code);
                return; // The launcher may have activated an existing instance.
            }
            var mainWindow = FindVisibleLauncherWindow(launcher.Id);
            if (mainWindow != IntPtr.Zero)
            {
                if (!IsIconic(mainWindow)) return;
                if (restored != mainWindow)
                {
                    ShowWindowAsync(mainWindow, 9); // SW_RESTORE
                    restored = mainWindow;
                }
            }
            await Task.Delay(40).ConfigureAwait(false);
        }
        throw new TimeoutException(T("等待 Lumi 窗口显示超时。", "Lumi did not show its window in time."));
    }

    private static IntPtr FindVisibleLauncherWindow(int processId)
    {
        var result = IntPtr.Zero;
        EnumWindows((window, unused) =>
        {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId || !IsWindowVisible(window)) return true;

            // The single-instance and Tao event windows are visible but have empty client areas.
            ClientRect bounds;
            if (!GetClientRect(window, out bounds) || bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top)
                return true;

            result = window;
            return false;
        }, IntPtr.Zero);
        return result;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ClientRect
    {
        public int Left, Top, Right, Bottom;
    }

    private delegate bool WindowVisitor(IntPtr window, IntPtr state);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(WindowVisitor visit, IntPtr state);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr window);

    [DllImport("user32.dll")]
    private static extern bool GetClientRect(IntPtr window, out ClientRect bounds);

    [DllImport("user32.dll")]
    private static extern bool IsIconic(IntPtr window);

    [DllImport("user32.dll")]
    private static extern bool ShowWindowAsync(IntPtr window, int command);

    private async Task Next()
    {
        if (busy || transitioning) return;
        if (succeeded)
        {
            if (uninstall) await CloseAfterMotion();
            else await OpenInstalledLauncher();
            return;
        }
        if (page == 0)
        {
            await SwitchWelcome(false);
            return;
        }
        Visible("PathError", false);
        if (!uninstall && !updating)
        {
            try
            {
                selectedInstall = ValidateDirectory(Find<TextBox>("ProgramPath").Text);
                selectedGames = ValidateDirectory(Find<TextBox>("GamePath").Text);
                if (String.Equals(selectedGames, selectedInstall, StringComparison.OrdinalIgnoreCase))
                    throw new ArgumentException(T("程序和游戏文件需要放在不同的文件夹。", "Use different folders for the launcher and game files."));
                CheckWritable(selectedInstall);
                CheckWritable(selectedGames);
            }
            catch (Exception error)
            {
                Text("PathError", error.Message);
                Visible("PathError", true);
                return;
            }
        }
        else selectedInstall = install;

        busy = true;
        Text("ProgressInstallPath", T("程序  ", "Launcher  ") + selectedInstall);
        Text("ProgressGamePath", uninstall || updating ? "" : T("游戏  ", "Games  ") + selectedGames);
        Text("ProgressText", uninstall ? T("正在移除启动器程序", "Removing the launcher") :
            updating ? T("正在更新启动器文件", "Updating launcher files") :
            T("正在安装启动器文件", "Installing launcher files"));
        Text("ProgressHint", T("请保持此窗口打开。", "Keep this window open."));
        var meter = Find<ProgressBar>("ProgressBar");
        lastStage = null;
        lastCheckpoint = 0;
        meter.Value = 0;
        await TransitionTo(() => Show("Progress", "",
            updating ? T("正在更新 Lumi", "Updating Lumi") :
                uninstall ? T("正在卸载", "Uninstalling") : T("正在安装 Lumi", "Installing Lumi"), "", "",
            "", ""));

        meter.ApplyTemplate();
        var fill = (Border)meter.Template.FindName("BarFill", meter);
        var scale = new ScaleTransform(0, 1);
        fill.RenderTransform = scale;

        string request = null;
        Exception failure = null;
        var timer = new System.Windows.Threading.DispatcherTimer();
        timer.Interval = TimeSpan.FromMilliseconds(180);
        timer.Tick += delegate
        {
            UpdateStage(meter, scale, ReadStatus(request));
        };
        try
        {
            request = NewRequest();
            progressGleam = StartGleam(meter);
            timer.Start();
            var status = await Task.Run(() => RunInstaller(request));
            var progress = ReadStatus(request);
            const string scanResultPrefix = "done:";
            bool scanReported = progress.StartsWith(scanResultPrefix, StringComparison.Ordinal);
            if (status != 0 || (progress != "done" && !scanReported))
            {
                var failureMessage = updating
                    ? T("更新未完成。请关闭正在运行的 Lumi 后重试。代码 ", "Update did not finish. Close any running Lumi window and retry. Code ")
                    : uninstall
                        ? T("卸载未完成。请关闭正在运行的 Lumi 后重试。代码 ", "Uninstall did not finish. Close any running Lumi window and retry. Code ")
                        : T("安装未完成。请关闭正在运行的 Lumi 后重试。代码 ", "Installation did not finish. Close any running Lumi window and retry. Code ");
                throw new InvalidOperationException(failureMessage + status);
            }
            if (uninstall && File.Exists(Path.Combine(install, binary)))
                throw new InvalidOperationException(T("程序仍在使用中。请关闭 Lumi 后重试。", "The launcher is still in use. Close Lumi and retry."));
            if (!uninstall && !File.Exists(Path.Combine(selectedInstall, binary)))
                throw new IOException(T("没有找到安装后的程序文件。", "The installed launcher file was not found."));
            timer.Stop();
            await UpdateStage(meter, scale, "done");
            succeeded = true;
            Result = 0;
            if (!uninstall)
            {
                if (updating)
                {
                    await CloseAfterMotion();
                }
                else if (Find<CheckBox>("LaunchChoice").IsChecked == true)
                {
                    await OpenInstalledLauncher();
                }
                else
                {
                    int foundGames = 0;
                    if (scanReported)
                        Int32.TryParse(progress.Substring(scanResultPrefix.Length), NumberStyles.None,
                            CultureInfo.InvariantCulture, out foundGames);
                    StopGleam();
                    busy = false;
                    await TransitionTo(() => ShowInstallComplete(foundGames));
                }
                return;
            }

            busy = false;
            bool cleanedSettings = Find<CheckBox>("CleanChoice").IsChecked == true;
            await TransitionTo(() =>
            {
                Text("SummaryTitle", cleanedSettings
                    ? T("Lumi 设置已清除，游戏文件已保留", "Lumi settings removed; game files kept")
                    : T("设置和游戏文件已保留", "Settings and game files were kept"));
                Text("SummaryText", T("已有实例、存档、截图和 Java 仍在原位置。", "Your instances, worlds, screenshots, and Java remain in their original locations."));
                Show("Summary", "", T("Lumi 已卸载", "Lumi is uninstalled"), "", "", T("完成", "Done"), "");
            });
        }
        catch (Exception error)
        {
            failure = error;
        }
        finally
        {
            timer.Stop();
            StopGleam();
            if (request != null)
            {
                try { Directory.Delete(Path.GetDirectoryName(request), true); }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
        }

        if (failure != null)
        {
            busy = false;
            page = 2;
            Result = 2;
            await TransitionTo(() =>
            {
                Text("SummaryTitle", updating ? T("更新未完成", "Update did not finish") :
                    uninstall ? T("卸载未完成", "Uninstall did not finish") :
                    T("安装未完成", "Installation did not finish"));
                Text("SummaryText", failure.Message);
                Show("Summary", "", T("请检查后重试", "Check and try again"), "", "",
                    T("重试", "Retry"), uninstall || updating ? T("取消", "Cancel") : T("上一步", "Back"));
            });
        }
    }

    private string ValidateDirectory(string value)
    {
        if (String.IsNullOrWhiteSpace(value) || !Path.IsPathRooted(value) ||
            (value.Length >= 2 && value[1] == ':' && (value.Length < 3 || value[2] != '\\')) ||
            value.IndexOfAny(new[] { '"', '\r', '\n' }) >= 0)
            throw new ArgumentException(T("请选择完整的文件夹路径。", "Choose a full folder path."));
        var full = Path.GetFullPath(value).TrimEnd('\\');
        if (String.Equals(full, Path.GetPathRoot(full + "\\").TrimEnd('\\'), StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException(T("请选择磁盘内的具体文件夹，不能直接选择盘符根目录。", "Choose a folder inside the drive instead of the drive root."));
        return full;
    }

    private void CheckWritable(string folder)
    {
        for (var parent = folder; !String.IsNullOrEmpty(parent); parent = Path.GetDirectoryName(parent))
        {
            FileAttributes attributes;
            try { attributes = File.GetAttributes(parent); }
            catch (FileNotFoundException) { continue; }
            catch (DirectoryNotFoundException) { continue; }

            if ((attributes & FileAttributes.Directory) == 0)
                throw new IOException(T("此路径已有同名文件，请选择其他文件夹。", "A file already exists at this path. Choose another folder."));

            // Probe the existing ancestor. The installer creates the selected folders when it needs them.
            var path = Path.Combine(parent, ".lumi-check-" + Guid.NewGuid().ToString("N"));
            using (new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None, 4096, FileOptions.DeleteOnClose)) { }
            return;
        }
        throw new DirectoryNotFoundException(T("找不到可访问的上级文件夹。", "No accessible parent folder was found."));
    }

    private string NewRequest()
    {
        var folder = Path.Combine(Path.GetTempPath(), "LumiSetup-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);
        var file = Path.Combine(folder, "request.txt");
        File.WriteAllLines(file, new[] {
            updating ? "update" : uninstall ? "remove" : "install",
            uninstall || updating ? "" : selectedGames,
            uninstall || updating || String.Equals(defaultGamePath, selectedGames, StringComparison.OrdinalIgnoreCase) ? "default" : "selected",
            Find<CheckBox>("DesktopChoice").IsChecked == true ? "1" : "0",
            Find<CheckBox>("PackChoice").IsChecked == true ? "1" : "0",
            Find<CheckBox>("CleanChoice").IsChecked == true ? "1" : "0"
        }, Encoding.Unicode);
        return file;
    }

    private static string ReadStatus(string request)
    {
        try { return File.ReadAllText(Path.Combine(Path.GetDirectoryName(request), "status.txt")).Trim(); }
        catch (IOException) { return ""; }
    }

    private int RunInstaller(string request)
    {
        var worker = engine;
        if (uninstall)
        {
            worker = Path.Combine(Path.GetDirectoryName(request), "uninstall-worker.exe");
            File.Copy(engine, worker);
        }
        var info = new ProcessStartInfo(worker) {
            Arguments = uninstall ? "/S /LUMI_WORKER _?=" + install :
                "/S /LUMI_WORKER " + (updating ? "/UPDATE " : "") + "/D=" + selectedInstall,
            WorkingDirectory = Path.GetDirectoryName(worker),
            UseShellExecute = false,
            CreateNoWindow = true
        };
        info.EnvironmentVariables["LUMI_SETUP_REQUEST"] = request;
        info.EnvironmentVariables["LUMI_SETUP_STATUS"] = Path.Combine(Path.GetDirectoryName(request), "status.txt");
        using (var process = Process.Start(info))
        {
            process.WaitForExit();
            return process.ExitCode;
        }
    }
}
