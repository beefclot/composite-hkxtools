use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, Frame};
use egui::{Color32, Context as EguiContext, RichText, Ui};
use rfd::FileDialog;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use futures::future::join_all;
use walkdir;

const HKXCMD_EXE: &[u8] = include_bytes!("hkxcmd.exe");
const HKXC_EXE: &[u8] = include_bytes!("hkxc.exe");
const HKXCONV_EXE: &[u8] = include_bytes!("hkxconv.exe");
const SSE_TO_LE_HKO: &[u8] = include_bytes!("_SSEtoLE.hko");
const HAVOK_BEHAVIOR_POST_PROCESS_EXE: &[u8] = include_bytes!("HavokBehaviorPostProcess.exe");
const HCT_STANDALONE_FILTER_MANAGER_EXE: &[u8] = include_bytes!("hctStandAloneFilterManager.exe");
const HCT_FILTER_MANAGER_DLL: &[u8] = include_bytes!("hctFilterManager.dll");

#[derive(PartialEq, Clone, Copy, Debug)]
enum ConverterTool {
    HkxCmd,
    Hct,
    HavokBehaviorPostProcess,
    HkxC,
    HkxConv,
}

impl ConverterTool {
    fn label(&self) -> &'static str {
        match self {
            ConverterTool::HkxCmd => "hkxcmd",
            ConverterTool::Hct => "HavokContentTools",
            ConverterTool::HavokBehaviorPostProcess => "HavokBehaviorPostProcess",
            ConverterTool::HkxC => "hkxc",
            ConverterTool::HkxConv => "hkxconv",
        }
    }

    /// Get help text for this tool
    fn help_text(&self) -> &'static str {
        match self {
            ConverterTool::HkxCmd => "LE animation HKX -> SE animation HKX || .kf || .xml (requires skeleton file)",
            ConverterTool::Hct => "SE animation HKX -> LE animation HKX",
            ConverterTool::HavokBehaviorPostProcess => "LE animation HKX -> SE animation HKX",
            ConverterTool::HkxC => "SE animation/behavior HKX <-> LE animation/behaviorHKX <-> .xml",
            ConverterTool::HkxConv => "SE behavior HKX <-> .xml",
        }
    }

    /// Check if this tool supports a given file extension
    fn supports_extension(&self, ext: &str) -> bool {
        match self {
            ConverterTool::HkxCmd => {
                matches!(ext, "hkx" | "xml" | "kf")
            }
            ConverterTool::HkxC | ConverterTool::HkxConv => {
                matches!(ext, "hkx" | "xml")
            }
            ConverterTool::Hct | ConverterTool::HavokBehaviorPostProcess => {
                matches!(ext, "hkx")
            }
        }
    }

    /// Check if this tool supports a given file path
    fn supports_file(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| self.supports_extension(ext))
            .unwrap_or(false)
    }

    /// Get available input file extensions for this tool
    fn available_input_extensions(&self) -> Vec<InputFileExtension> {
        match self {
            ConverterTool::HkxCmd => {
                vec![
                    InputFileExtension::All,
                    InputFileExtension::Hkx,
                    InputFileExtension::Xml,
                    InputFileExtension::Kf,
                ]
            }
            ConverterTool::HkxC | ConverterTool::HkxConv => {
                vec![
                    InputFileExtension::All,
                    InputFileExtension::Hkx,
                    InputFileExtension::Xml,
                ]
            }
            ConverterTool::Hct | ConverterTool::HavokBehaviorPostProcess => {
                vec![
                    InputFileExtension::All,
                    InputFileExtension::Hkx,
                ]
            }
        }
    }

    /// Get available output formats for this tool
    fn available_output_formats(&self) -> Vec<OutputFormat> {
        match self {
            ConverterTool::HkxCmd => {
                vec![
                    OutputFormat::Xml,
                    OutputFormat::SkyrimLE,
                    OutputFormat::SkyrimSE,
                    OutputFormat::Kf,
                ]
            }
            ConverterTool::HkxC => {
                vec![
                    OutputFormat::Xml,
                    OutputFormat::SkyrimLE,
                    OutputFormat::SkyrimSE,
                ]
            }
            ConverterTool::HkxConv => {
                vec![
                    OutputFormat::Xml,
                    OutputFormat::SkyrimSE,
                ]
            }
            ConverterTool::Hct => {
                vec![OutputFormat::SkyrimLE]
            }
            ConverterTool::HavokBehaviorPostProcess => {
                vec![OutputFormat::SkyrimSE]
            }
        }
    }

    /// Get supported formats description for drag & drop overlay
    fn supported_formats_description(&self) -> &'static str {
        match self {
            ConverterTool::HkxCmd => "Supports: HKX, XML, KF files",
            ConverterTool::HkxC | ConverterTool::HkxConv => "Supports: HKX, XML files",
            ConverterTool::Hct | ConverterTool::HavokBehaviorPostProcess => "Supports: HKX files",
        }
    }
}

#[derive(Debug, Clone)]
enum ConversionStatus {
    Idle,
    Running { current_file: String, progress: usize, total: usize },
    Completed { message: String },
    Error { message: String },
}

#[derive(Debug)]
struct ConversionProgress {
    current_file: String,
    file_index: usize,
    total_files: usize,
    status: ConversionStatus,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum InputFileExtension {
    All,
    Hkx,
    Xml,
    Kf,
}

impl InputFileExtension {
    fn label_for_tool(&self, tool: ConverterTool) -> &'static str {
        match self {
            InputFileExtension::All => match tool {
                ConverterTool::HkxCmd => "All (HKX, XML, KF)",
                ConverterTool::HkxC => "All (HKX, XML)",
                ConverterTool::HkxConv => "All (HKX, XML)",
                ConverterTool::Hct => "All (HKX only)",
                ConverterTool::HavokBehaviorPostProcess => "All (HKX only)",
            },
            InputFileExtension::Hkx => "HKX only",
            InputFileExtension::Xml => "XML only",
            InputFileExtension::Kf => "KF only",
        }
    }
}

struct HkxToolsApp {
    input_paths: Vec<PathBuf>,
    output_folder: Option<PathBuf>,
    skeleton_file: Option<PathBuf>,
    output_suffix: String,
    output_format: OutputFormat,
    custom_extension: Option<String>,
    input_file_extension: InputFileExtension,
    converter_tool: ConverterTool,
    hkxcmd_path: PathBuf,
    hkxc_path: PathBuf,
    hkxconv_path: PathBuf,
    sse_to_le_hko_path: PathBuf,
    havok_behavior_post_process_path: PathBuf,
    hct_standalone_filter_manager_path: PathBuf,
    hct_filter_manager_dll_path: PathBuf,
    // Track base folder for relative path calculations
    base_folder: Option<PathBuf>,
    // Track if output folder was manually set by user
    output_folder_manually_set: bool,
    // Async operation fields
    conversion_status: ConversionStatus,
    progress_rx: Option<mpsc::UnboundedReceiver<ConversionProgress>>,
    cancel_tx: Option<oneshot::Sender<()>>,
    tokio_handle: tokio::runtime::Handle,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum OutputFormat {
    Xml,
    SkyrimLE,
    SkyrimSE,
    Kf,
}

impl OutputFormat {
    fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Xml => "xml",
            OutputFormat::SkyrimLE | OutputFormat::SkyrimSE => "hkx",
            OutputFormat::Kf => "kf",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            OutputFormat::Xml => "XML",
            OutputFormat::SkyrimLE => "Skyrim LE",
            OutputFormat::SkyrimSE => "Skyrim SE",
            OutputFormat::Kf => "KF",
        }
    }

    /// Check if this output format requires a skeleton file
    fn requires_skeleton(&self) -> bool {
        matches!(self, OutputFormat::Kf)
    }
}

impl Default for HkxToolsApp {
    fn default() -> Self {
        Self {
            input_paths: Vec::new(),
            output_folder: None,
            skeleton_file: None,
            output_suffix: String::new(),
            output_format: OutputFormat::Xml,
            custom_extension: None,
            input_file_extension: InputFileExtension::All,
            converter_tool: ConverterTool::HkxCmd,
            hkxcmd_path: PathBuf::new(),
            hkxc_path: PathBuf::new(),
            hkxconv_path: PathBuf::new(),
            sse_to_le_hko_path: PathBuf::new(),
            havok_behavior_post_process_path: PathBuf::new(),
            hct_standalone_filter_manager_path: PathBuf::new(),
            hct_filter_manager_dll_path: PathBuf::new(),
            base_folder: None,
            output_folder_manually_set: false,
            conversion_status: ConversionStatus::Idle,
            progress_rx: None,
            cancel_tx: None,
            tokio_handle: tokio::runtime::Handle::current(),
        }
    }
}

// Temporary context for async conversion operations
struct TempConversionContext {
    converter_tool: ConverterTool,
    output_format: OutputFormat,
    skeleton_file: Option<PathBuf>,
    hkxcmd_path: PathBuf,
    hkxc_path: PathBuf,
    hkxconv_path: PathBuf,
    sse_to_le_hko_path: PathBuf,
    havok_behavior_post_process_path: PathBuf,
    hct_standalone_filter_manager_path: PathBuf,
    hct_filter_manager_dll_path: PathBuf,
}

impl TempConversionContext {
    async fn run_conversion_tool(&self, input: &Path, output: &Path) -> Result<()> {
        let mut command = match self.converter_tool {
            ConverterTool::HkxCmd => Command::new(&self.hkxcmd_path),
            ConverterTool::Hct => Command::new(&self.hct_standalone_filter_manager_path),
            ConverterTool::HavokBehaviorPostProcess => Command::new(&self.havok_behavior_post_process_path),
            ConverterTool::HkxC => Command::new(&self.hkxc_path),
            ConverterTool::HkxConv => Command::new(&self.hkxconv_path),
        };
        
        let tool_name = match self.converter_tool {
            ConverterTool::HkxCmd => "hkxcmd",
            ConverterTool::Hct => "hctStandAloneFilterManager",
            ConverterTool::HavokBehaviorPostProcess => "HavokBehaviorPostProcess",
            ConverterTool::HkxC => "hkxc",
            ConverterTool::HkxConv => "hkxconv",
        };

        // Convert paths to absolute paths to avoid issues with paths starting with '-'
        // Use absolute paths but avoid canonicalize() which can add \\?\ prefix on Windows
        let input_absolute = HkxToolsApp::ensure_absolute_path(input);
        let output_absolute = HkxToolsApp::ensure_absolute_path(output);
        
        // Also handle skeleton file if it exists
        let skeleton_absolute = self.skeleton_file.as_ref().map(|skeleton| {
            HkxToolsApp::ensure_absolute_path(skeleton)
        });
        
        // Set the command based on output format
        if self.output_format == OutputFormat::Kf {
            if self.converter_tool != ConverterTool::Hct {
                // For KF output, we need to determine direction based on input file extension
                let input_ext = input_absolute.extension().and_then(|ext| ext.to_str()).unwrap_or("");
                if input_ext == "kf" {
                    command.arg("ConvertKF"); // KF -> HKX
                } else {
                    command.arg("exportkf"); // HKX -> KF
                }
            }
            // HCT doesn't support KF conversion
        } else {
            if self.converter_tool != ConverterTool::Hct && self.converter_tool != ConverterTool::HavokBehaviorPostProcess {
                command.arg("convert");
            }
            // HCT and HavokBehaviorPostProcess don't need a command argument
        }

        // Add arguments based on tool and output format
        match self.converter_tool {
            ConverterTool::HkxCmd => {
                if self.output_format == OutputFormat::Kf {
                    // KF conversion
                    if let Some(skeleton) = &skeleton_absolute {
                        command.arg(skeleton);
                    }
                    command.arg(&input_absolute);
                    command.arg(&output_absolute);
                    // For HKX <> KF, determine if we need version argument based on direction
                    let input_ext = input_absolute.extension().and_then(|ext| ext.to_str()).unwrap_or("");
                    if input_ext == "kf" {
                        // KF -> HKX conversion
                        command.arg(format!("-v:{}", match self.output_format {
                            OutputFormat::Xml => "XML",
                            OutputFormat::SkyrimLE => "WIN32",
                            OutputFormat::SkyrimSE => "AMD64",
                            OutputFormat::Kf => "AMD64",
                        }));
                    }
                    // HKX -> KF doesn't need version argument
                } else {
                    // Regular HKX/XML conversion
                    command.arg("-i").arg(&input_absolute);
                    command.arg("-o").arg(&output_absolute);
                    command.arg(format!("-v:{}", match self.output_format {
                        OutputFormat::Xml => "XML",
                        OutputFormat::SkyrimLE => "WIN32",
                        OutputFormat::SkyrimSE => "AMD64",
                        OutputFormat::Kf => "AMD64", // This shouldn't happen in regular conversion
                    }));
                }
            }
            ConverterTool::HkxC => {
                if self.output_format == OutputFormat::Kf {
                    return Err(anyhow::anyhow!("hkxc does not support KF conversion"));
                }
                command.arg("--input").arg(&input_absolute);
                command.arg("--output").arg(&output_absolute);
                command.arg("--format").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "win32",
                    OutputFormat::SkyrimSE => "amd64",
                    OutputFormat::Kf => "amd64", // This shouldn't happen
                });
            }
            ConverterTool::HkxConv => {
                if self.output_format == OutputFormat::Kf {
                    return Err(anyhow::anyhow!("hkxconv does not support KF conversion"));
                }
                command.arg(&input_absolute);
                command.arg(&output_absolute);
                command.arg("-v").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "hkx",
                    OutputFormat::SkyrimSE => "hkx",
                    OutputFormat::Kf => "hkx", // This shouldn't happen
                });
            }
            ConverterTool::Hct => {
                if self.output_format == OutputFormat::Kf {
                    return Err(anyhow::anyhow!("HCT does not support KF conversion"));
                }
                
                // For HCT, create a unique temporary directory for this conversion
                let temp_dir = tempfile::Builder::new()
                    .prefix("hct_conversion_")
                    .tempdir()
                    .context("Failed to create temporary directory for HCT conversion")?;
                
                // HCT only supports SSE to LE conversion
                let source_hko_path = &self.sse_to_le_hko_path;
                
                // Copy the .hko file to the temporary directory
                let hko_filename = source_hko_path.file_name().unwrap();
                let temp_hko_path = temp_dir.path().join(hko_filename);
                fs::copy(source_hko_path, &temp_hko_path)
                    .context("Failed to copy .hko file to temporary directory")?;
                
                println!("HCT temp dir: {:?}, using .hko: {:?}", temp_dir.path(), hko_filename);
                
                // Set working directory to temp directory and use relative .hko filename
                command.current_dir(temp_dir.path());
                command.arg(&input_absolute);
                command.arg("-s");
                command.arg(hko_filename);  // Just the filename, not full path
                
                // Execute the command
                let cmd_output = command.output().await.context("Failed to execute HCT converter tool")?;
                let stderr = String::from_utf8_lossy(&cmd_output.stderr);

                if !cmd_output.status.success() {
                    return Err(anyhow::anyhow!("{} failed: {}", tool_name, stderr));
                }
                
                // HCT creates "filename.hkx" in the same directory as the .hko file
                let hct_output_file = temp_dir.path().join("filename.hkx");
                
                // Debug: List all files in temp directory
                println!("Temp directory contents:");
                if let Ok(entries) = fs::read_dir(temp_dir.path()) {
                    for entry in entries.flatten() {
                        println!("  {:?}", entry.path());
                    }
                } else {
                    println!("  Failed to read temp directory");
                }
                
                if !hct_output_file.exists() {
                    return Err(anyhow::anyhow!("HCT did not produce expected output file: {:?}", hct_output_file));
                }
                
                println!("HCT output file exists: {:?}", hct_output_file);
                println!("Target output path: {:?}", output_absolute);
                
                // Create output directory if it doesn't exist
                if let Some(parent) = output_absolute.parent() {
                    println!("Creating output directory: {:?}", parent);
                    fs::create_dir_all(parent).context("Failed to create output directory")?;
                }
                
                // Check if target file already exists and remove it if necessary
                if output_absolute.exists() {
                    println!("Target file already exists, removing: {:?}", output_absolute);
                    fs::remove_file(&output_absolute).context("Failed to remove existing target file")?;
                }
                
                // Move the HCT output file directly to the final location
                // The output_absolute path already includes any suffix/extension modifications
                match fs::rename(&hct_output_file, &output_absolute) {
                    Ok(_) => {
                        println!("Successfully moved HCT output to: {:?}", output_absolute);
                    }
                    Err(e) => {
                        // If rename fails, try copy + delete as fallback
                        println!("Rename failed ({}), trying copy + delete fallback", e);
                        fs::copy(&hct_output_file, &output_absolute)
                            .context("Failed to copy HCT output file to final location")?;
                        fs::remove_file(&hct_output_file)
                            .context("Failed to remove temporary HCT output file after copy")?;
                        println!("Successfully copied HCT output to: {:?}", output_absolute);
                    }
                }
                
                println!("HCT conversion complete: {:?} -> {:?}", input_absolute, output_absolute);
                
                // temp_dir will be automatically cleaned up when it goes out of scope
                return Ok(());
            }
            ConverterTool::HavokBehaviorPostProcess => {
                if self.output_format == OutputFormat::Kf {
                    return Err(anyhow::anyhow!("HavokBehaviorPostProcess does not support KF conversion"));
                }
                
                // HavokBehaviorPostProcess only supports HKX input files and SSE output
                if input_absolute.extension().map_or(true, |ext| ext != "hkx") {
                    return Err(anyhow::anyhow!("HavokBehaviorPostProcess requires an HKX input file."));
                }
                
                // HavokBehaviorPostProcess modifies files in-place, so we need to copy the input to output first
                println!("Input path: {:?}", input_absolute);
                println!("Output path: {:?}", output_absolute);
                println!("Input exists: {}", input_absolute.exists());
                println!("Output parent exists: {}", output_absolute.parent().map_or(false, |p| p.exists()));
                println!("Copying input file to output location: {:?} -> {:?}", input_absolute, output_absolute);
                
                // Check if input and output are the same
                if input_absolute == output_absolute {
                    return Err(anyhow::anyhow!("Input and output paths are the same: {:?}", input_absolute));
                }
                
                // Create output directory if it doesn't exist
                if let Some(parent) = output_absolute.parent() {
                    println!("Creating output directory: {:?}", parent);
                    fs::create_dir_all(parent).context("Failed to create output directory")?;
                }
                
                // Copy input file to output location
                match fs::copy(&input_absolute, &output_absolute) {
                    Ok(bytes_copied) => {
                        println!("Successfully copied {} bytes", bytes_copied);
                    }
                    Err(e) => {
                        println!("Copy failed with error: {:?}", e);
                        return Err(anyhow::anyhow!("Failed to copy input file to output location: {}", e));
                    }
                }
                
                // Check file size before processing
                let file_size_before = fs::metadata(&output_absolute)
                    .context("Failed to get file metadata before processing")?
                    .len();
                println!("File size before HavokBehaviorPostProcess: {} bytes", file_size_before);
                
                // Run HavokBehaviorPostProcess on the output file (modifies in-place)
                command.arg("--platformAmd64");
                // Both input and output are the same file (in-place modification)
                // Don't manually add quotes - let Command handle it
                command.arg(&output_absolute);
                command.arg(&output_absolute);
            }
        }

        // Print the command being executed for debugging
        println!("EXECUTING COMMAND: {:?} with input: {:?}, output: {:?}", tool_name, input_absolute, output_absolute);
        
        // For HavokBehaviorPostProcess, print the exact command with arguments
        if self.converter_tool == ConverterTool::HavokBehaviorPostProcess {
            println!("HavokBehaviorPostProcess command: {:?}", command);
        }

        let output = command.output().await.context("Failed to execute converter tool")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        
        // For HavokBehaviorPostProcess, print all output for debugging
        if self.converter_tool == ConverterTool::HavokBehaviorPostProcess {
            println!("HavokBehaviorPostProcess exit code: {:?}", output.status.code());
            println!("HavokBehaviorPostProcess stdout: {}", stdout);
            println!("HavokBehaviorPostProcess stderr: {}", stderr);
        }

        if !output.status.success() {
            return Err(anyhow::anyhow!("{} failed with exit code {:?}: stdout: {} stderr: {}", 
                tool_name, output.status.code(), stdout, stderr));
        }
        
        // For HavokBehaviorPostProcess, check if the file size changed
        if self.converter_tool == ConverterTool::HavokBehaviorPostProcess {
            let file_size_after = fs::metadata(&output_absolute)
                .context("Failed to get file metadata after processing")?
                .len();
            println!("File size after HavokBehaviorPostProcess: {} bytes", file_size_after);
            
            if file_size_after == fs::metadata(&input_absolute)
                .context("Failed to get input file metadata")?
                .len() {
                println!("WARNING: Output file size is the same as input file size - conversion may not have worked");
            } else {
                println!("SUCCESS: File size changed, conversion appears to have worked");
            }
        }

        Ok(())
    }
}

impl HkxToolsApp {
    fn new(hkxcmd_path: PathBuf, hkxc_path: PathBuf, hkxconv_path: PathBuf, sse_to_le_hko_path: PathBuf, havok_behavior_post_process_path: PathBuf, hct_standalone_filter_manager_path: PathBuf, hct_filter_manager_dll_path: PathBuf, tokio_handle: tokio::runtime::Handle) -> Self {
        Self {
            input_paths: Vec::new(),
            output_folder: None,
            skeleton_file: None,
            output_suffix: String::new(),
            output_format: OutputFormat::Xml,
            custom_extension: None,
            input_file_extension: InputFileExtension::All,
            converter_tool: ConverterTool::HkxCmd,
            hkxcmd_path,
            hkxc_path,
            hkxconv_path,
            sse_to_le_hko_path,
            havok_behavior_post_process_path,
            hct_standalone_filter_manager_path,
            hct_filter_manager_dll_path,
            base_folder: None,
            output_folder_manually_set: false,
            conversion_status: ConversionStatus::Idle,
            progress_rx: None,
            cancel_tx: None,
            tokio_handle,
        }
    }

    /// Check if a file matches the current input filter and tool capabilities
    fn file_matches_filter(&self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }

        match self.input_file_extension {
            InputFileExtension::All => self.converter_tool.supports_file(path),
            InputFileExtension::Hkx => {
                path.extension().map_or(false, |ext| ext == "hkx")
            }
            InputFileExtension::Xml => {
                path.extension().map_or(false, |ext| ext == "xml")
            }
            InputFileExtension::Kf => {
                path.extension().map_or(false, |ext| ext == "kf")
            }
        }
    }

    /// Create absolute path from relative path
    fn ensure_absolute_path(path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    }

    /// Open a folder in the system file explorer
    fn open_folder_in_explorer(folder_path: &Path) {
        #[cfg(target_os = "windows")]
        {
            if let Err(e) = std::process::Command::new("explorer")
                .arg(folder_path)
                .spawn()
            {
                eprintln!("Failed to open folder in explorer: {}", e);
            }
        }
        
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = std::process::Command::new("open")
                .arg(folder_path)
                .spawn()
            {
                eprintln!("Failed to open folder in Finder: {}", e);
            }
        }
        
        #[cfg(target_os = "linux")]
        {
            if let Err(e) = std::process::Command::new("xdg-open")
                .arg(folder_path)
                .spawn()
            {
                eprintln!("Failed to open folder in file manager: {}", e);
            }
        }
    }

    /// Show a tooltip for a converter tool
    fn show_tool_tooltip(&self, ui: &mut Ui, tool: ConverterTool, hover_pos: egui::Pos2) {
        let tooltip_text = tool.help_text();
        
        // Get screen bounds to ensure tooltip doesn't go off-screen
        let screen_rect = ui.ctx().screen_rect();
        
        // Calculate dynamic tooltip width based on available space
        let max_tooltip_width = 300.0; // Maximum width
        let min_tooltip_width = 200.0; // Minimum width
        let available_width = screen_rect.width() - 40.0; // Leave 20px margin on each side
        
        // Choose the best width: available space, but not less than minimum or more than maximum
        let tooltip_width = available_width
            .max(min_tooltip_width)
            .min(max_tooltip_width);
        
        // Calculate position for the tooltip with better positioning logic
        let mut tooltip_pos = egui::Pos2::new(hover_pos.x + 20.0, hover_pos.y - 10.0);
        
        // Ensure tooltip doesn't go off the right edge of the screen
        if tooltip_pos.x + tooltip_width > screen_rect.right() {
            tooltip_pos.x = hover_pos.x - tooltip_width - 20.0;
        }
        
        // Calculate available vertical space above and below the hover position
        let space_above = hover_pos.y - screen_rect.top();
        let space_below = screen_rect.bottom() - hover_pos.y;
        
        // Estimate tooltip height (will be calculated more accurately by the layout)
        let estimated_height = 120.0;
        
        // Choose the best vertical position based on available space
        if space_below >= estimated_height {
            // Position below the hover point (default)
            tooltip_pos.y = hover_pos.y + 10.0;
        } else if space_above >= estimated_height {
            // Position above the hover point
            tooltip_pos.y = hover_pos.y - estimated_height - 10.0;
        } else {
            // Not enough space in either direction, position to maximize visibility
            if space_below > space_above {
                // More space below, position at bottom edge
                tooltip_pos.y = screen_rect.bottom() - estimated_height - 10.0;
            } else {
                // More space above, position at top edge
                tooltip_pos.y = screen_rect.top() + 10.0;
            }
        }
        
        // Final bounds checking to ensure tooltip is fully visible
        if tooltip_pos.y < screen_rect.top() {
            tooltip_pos.y = screen_rect.top() + 10.0;
        }
        if tooltip_pos.y + estimated_height > screen_rect.bottom() {
            tooltip_pos.y = screen_rect.bottom() - estimated_height - 10.0;
        }
        
        // Create a tooltip area
        egui::Area::new("tool_tooltip".into())
            .fixed_pos(tooltip_pos)
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                // Background panel with dynamic width for proper wrapping
                egui::Frame::none()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().strong_text_color()))
                    .rounding(4.0)
                    .show(ui, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(tooltip_width, 0.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.add_space(8.0);
                                
                                // Tool name with left and right margins
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(tool.label())
                                            .size(14.0)
                                            .strong()
                                            .color(ui.visuals().strong_text_color())
                                    );
                                    ui.add_space(8.0);
                                });
                                
                                ui.add_space(4.0);
                                
                                // Help text with left and right margins and wrapping
                                ui.horizontal(|ui| {
                                    ui.add_space(8.0);
                                    // Constrain the label width to force text wrapping
                                    ui.allocate_ui_with_layout(
                                        egui::Vec2::new(tooltip_width - 16.0, 0.0), // Subtract margins
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                            ui.label(
                                                RichText::new(tooltip_text)
                                                    .size(12.0)
                                                    .color(ui.visuals().text_color())
                                            );
                                        }
                                    );
                                    ui.add_space(8.0);
                                });
                                
                                ui.add_space(8.0);
                            }
                        );
                    });
            });
    }

    /// Get available output formats for the current tool
    fn available_output_formats(&self) -> Vec<OutputFormat> {
        self.converter_tool.available_output_formats()
    }

    fn add_files_from_folder(&mut self, folder: &Path, recursive: bool) -> Result<()> {
        // Set the base folder for relative path calculations
        self.base_folder = Some(folder.to_path_buf());
        
        if recursive {
            self.add_files_recursive(folder)
        } else {
            self.add_files_non_recursive(folder)
        }
    }

    fn add_files_non_recursive(&mut self, folder: &Path) -> Result<()> {
        let entries = fs::read_dir(folder).context("Failed to read directory")?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if self.file_matches_filter(&path) && !self.input_paths.contains(&path) {
                self.input_paths.push(path);
            }
        }
        Ok(())
    }

    fn add_files_recursive(&mut self, folder: &Path) -> Result<()> {
        for entry in walkdir::WalkDir::new(folder).follow_links(true) {
            let entry = entry?;
            let path = entry.path().to_path_buf();
            if self.file_matches_filter(&path) && !self.input_paths.contains(&path) {
                self.input_paths.push(path);
            }
        }
        Ok(())
    }

    fn update_output_folder(&mut self) {
        // Only update output folder if it hasn't been manually set by the user
        if !self.output_folder_manually_set {
            if let Some(input_path) = self.input_paths.first() {
                self.output_folder = Some(input_path.parent().unwrap_or(Path::new("")).to_path_buf());
            }
        }
    }

    /// Add a single file to the input files list, checking if it matches the current extension filter
    fn add_file(&mut self, file_path: PathBuf) -> bool {
        if self.file_matches_filter(&file_path) && !self.input_paths.contains(&file_path) {
            self.input_paths.push(file_path);
            true
        } else {
            false
        }
    }

    /// Process dropped files and add valid ones to the input files list
    fn handle_dropped_files(&mut self, dropped_files: Vec<egui::DroppedFile>) {
        let mut files_added = 0;
        let mut files_skipped = 0;

        for dropped_file in dropped_files {
            if let Some(path) = dropped_file.path {
                if path.is_file() {
                    if self.add_file(path) {
                        files_added += 1;
                    } else {
                        files_skipped += 1;
                    }
                } else if path.is_dir() {
                    // If a directory is dropped, add all files from it (non-recursive)
                    // Set the base folder for relative path calculations
                    self.base_folder = Some(path.clone());
                    if let Ok(entries) = std::fs::read_dir(&path) {
                        for entry in entries.flatten() {
                            let entry_path = entry.path();
                            if entry_path.is_file() {
                                if self.add_file(entry_path) {
                                    files_added += 1;
                                } else {
                                    files_skipped += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Update output folder if files were added
        if files_added > 0 {
            self.update_output_folder();
        }

        // Print feedback for debugging
        if files_added > 0 || files_skipped > 0 {
            println!("Drag & Drop: Added {} files, skipped {} files", files_added, files_skipped);
        }
    }

    /// Render a visual overlay when files are being dragged over the window
    fn render_drag_drop_overlay(&self, ctx: &EguiContext, hovered_files_count: usize) {
        // Create a semi-transparent overlay covering the entire window
        egui::Area::new("drag_drop_overlay".into())
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                // Get the available screen space
                let screen_rect = ctx.screen_rect();
                
                // Draw semi-transparent background
                ui.allocate_ui_at_rect(screen_rect, |ui| {
                    // Background with semi-transparent blue
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::Rounding::ZERO,
                        Color32::from_rgba_unmultiplied(0, 100, 200, 100), // Semi-transparent blue
                    );
                    
                    // Add animated dashed border for better visual feedback
                    let border_color = Color32::from_rgb(0, 150, 255);
                    let border_width = 4.0;
                    
                    // Create a dashed border effect by drawing multiple smaller rectangles
                    let margin = border_width / 2.0;
                    let inner_rect = screen_rect.shrink(margin);
                    
                    // Draw the main border
                    ui.painter().rect_stroke(
                        inner_rect,
                        egui::Rounding::same(5.0),
                        egui::Stroke::new(border_width, border_color),
                    );
                    
                    // Add an inner glow effect with a slightly smaller rectangle
                    let glow_rect = inner_rect.shrink(border_width);
                    ui.painter().rect_stroke(
                        glow_rect,
                        egui::Rounding::same(5.0),
                        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 150, 255, 150)),
                    );
                    
                    // Center the content
                    ui.allocate_ui_at_rect(screen_rect, |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.vertical_centered(|ui| {
                                // Create a centered box for the content
                                ui.allocate_ui_with_layout(
                                    egui::Vec2::new(400.0, 300.0),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(20.0);
                                        
                                        // Large drop icon with background
                                        ui.label(RichText::new("⬇").size(80.0).color(Color32::WHITE));
                                        
                                        ui.add_space(15.0);
                                        
                                        // Main drop message
                                        ui.label(
                                            RichText::new("Drop Files Here")
                                                .size(28.0)
                                                .color(Color32::WHITE)
                                                .strong()
                                        );
                                        
                                        ui.add_space(15.0);
                                        
                                        // File count and supported formats
                                        let file_text = if hovered_files_count == 1 {
                                            "1 file ready to drop".to_string()
                                        } else {
                                            format!("{} files ready to drop", hovered_files_count)
                                        };
                                        
                                        ui.label(
                                            RichText::new(file_text)
                                                .size(18.0)
                                                .color(Color32::from_rgb(200, 230, 255))
                                        );
                                        
                                        ui.add_space(10.0);
                                        
                                                                // Supported formats
                        let supported_formats = self.converter_tool.supported_formats_description();
                                        
                                        ui.label(
                                            RichText::new(supported_formats)
                                                .size(14.0)
                                                .color(Color32::from_rgb(180, 210, 255))
                                                .italics()
                                        );
                                        
                                        ui.add_space(10.0);
                                        
                                        // Add a subtle hint about folder support
                                        ui.label(
                                            RichText::new("Files and folders are supported")
                                                .size(12.0)
                                                .color(Color32::from_rgb(150, 180, 220))
                                                .italics()
                                        );
                                    }
                                );
                            });
                        });
                    });
                });
            });
    }

    fn get_output_path(&self, input_path: &Path) -> Option<PathBuf> {
        let output_base = self.output_folder.as_ref()?;
        let file_name = input_path.file_stem()?.to_str()?;
        
        // Determine output extension based on output format and custom extension
        let extension = if let Some(custom_ext) = &self.custom_extension {
            custom_ext.as_str()
        } else {
            self.output_format.extension()
        };

        // Calculate relative path from base folder to maintain folder structure
        let relative_path = if let Some(base_folder) = &self.base_folder {
            // If we have a base folder, calculate relative path from it
            if let Ok(relative) = input_path.parent().unwrap_or(Path::new("")).strip_prefix(base_folder) {
                relative.to_path_buf()
            } else {
                // Fallback: use the parent directory relative to the input path
                input_path.parent().unwrap_or(Path::new("")).to_path_buf()
            }
        } else {
            // Fallback to old behavior for single files or when no base folder is set
            let base_dir = if self.input_paths.len() == 1 {
                input_path.parent().unwrap_or(Path::new(""))
            } else {
                self.find_common_parent_dir()
                    .unwrap_or_else(|| Path::new(""))
            };

            input_path
                .parent()
                .unwrap_or(Path::new(""))
                .strip_prefix(base_dir)
                .unwrap_or(Path::new(""))
                .to_path_buf()
        };

        let output_name = if self.output_suffix.is_empty() {
            format!("{}.{}", file_name, extension)
        } else {
            format!("{}_{}.{}", file_name, self.output_suffix, extension)
        };

        Some(output_base.join(relative_path).join(output_name))
    }

    fn find_common_parent_dir(&self) -> Option<&Path> {
        if self.input_paths.is_empty() {
            return None;
        }

        // get all parent directories
        let parent_dirs: Vec<_> = self
            .input_paths
            .iter()
            .filter_map(|path| path.parent())
            .collect();

        if parent_dirs.is_empty() {
            return None;
        }

        // start with the first parent directory
        let mut common = parent_dirs[0];

        // find the common prefix among all parent directories
        for dir in &parent_dirs[1..] {
            while !dir.starts_with(common) {
                common = common.parent()?;
            }
        }

        Some(common)
    }

    fn start_conversion(&mut self) {
        // Validation
        if self.input_paths.is_empty() {
            self.conversion_status = ConversionStatus::Error {
                message: "No input files selected".to_string(),
            };
            return;
        }
        if self.output_folder.is_none() {
            self.conversion_status = ConversionStatus::Error {
                message: "No output folder selected".to_string(),
            };
            return;
        }
        if self.output_format.requires_skeleton() && self.skeleton_file.is_none() {
            self.conversion_status = ConversionStatus::Error {
                message: "Skeleton file is required for KF conversion".to_string(),
            };
            return;
        }

        // Setup channels for progress communication
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        
        self.progress_rx = Some(progress_rx);
        self.cancel_tx = Some(cancel_tx);
        self.conversion_status = ConversionStatus::Running {
            current_file: "Starting...".to_string(),
            progress: 0,
            total: self.input_paths.len(),
        };

        // Clone data needed for the async task
        let input_paths = self.input_paths.clone();
        let output_folder = self.output_folder.clone().unwrap();
        let skeleton_file = self.skeleton_file.clone();
        let output_suffix = self.output_suffix.clone();
        let output_format = self.output_format;
        let custom_extension = self.custom_extension.clone();
        let converter_tool = self.converter_tool;
        let hkxcmd_path = self.hkxcmd_path.clone();
        let hkxc_path = self.hkxc_path.clone();
        let hkxconv_path = self.hkxconv_path.clone();
        let sse_to_le_hko_path = self.sse_to_le_hko_path.clone();
        let havok_behavior_post_process_path = self.havok_behavior_post_process_path.clone();
        let hct_standalone_filter_manager_path = self.hct_standalone_filter_manager_path.clone();
        let hct_filter_manager_dll_path = self.hct_filter_manager_dll_path.clone();
        let base_folder = self.base_folder.clone();

        // Spawn the async conversion task
        self.tokio_handle.spawn(async move {
            let result = Self::run_conversion_async(
                input_paths,
                output_folder,
                skeleton_file,
                output_suffix,
                output_format,
                custom_extension,
                converter_tool,
                hkxcmd_path,
                hkxc_path,
                hkxconv_path,
                sse_to_le_hko_path,
                havok_behavior_post_process_path,
                hct_standalone_filter_manager_path,
                hct_filter_manager_dll_path,
                base_folder,
                progress_tx,
                cancel_rx,
            ).await;

            // The task will complete on its own
            drop(result);
        });
    }

    async fn run_conversion_async(
        input_paths: Vec<PathBuf>,
        output_folder: PathBuf,
        skeleton_file: Option<PathBuf>,
        output_suffix: String,
        output_format: OutputFormat,
        custom_extension: Option<String>,
        converter_tool: ConverterTool,
        hkxcmd_path: PathBuf,
        hkxc_path: PathBuf,
        hkxconv_path: PathBuf,
        sse_to_le_hko_path: PathBuf,
        havok_behavior_post_process_path: PathBuf,
        hct_standalone_filter_manager_path: PathBuf,
        hct_filter_manager_dll_path: PathBuf,
        base_folder: Option<PathBuf>,
        progress_tx: mpsc::UnboundedSender<ConversionProgress>,
        mut cancel_rx: oneshot::Receiver<()>,
    ) -> Result<()> {
        let total_files = input_paths.len();
        
        // HCT can now process asynchronously with isolated temp directories
        println!("Processing {} files with {}", total_files, match converter_tool {
            ConverterTool::Hct => "HCT (using isolated temp directories)",
            ConverterTool::HavokBehaviorPostProcess => "HavokBehaviorPostProcess",
            _ => "concurrent processing"
        });
        let mut conversion_tasks = Vec::new();
        
        for (index, input_path) in input_paths.iter().enumerate() {
            // Check for cancellation before starting
            if cancel_rx.try_recv().is_ok() {
                let _ = progress_tx.send(ConversionProgress {
                    current_file: "Cancelled".to_string(),
                    file_index: index,
                    total_files,
                    status: ConversionStatus::Error {
                        message: "Conversion cancelled by user".to_string(),
                    },
                });
                return Ok(());
            }

            let output_path = Self::get_output_path_static(
                input_path,
                &output_folder,
                &output_suffix,
                output_format,
                &custom_extension,
                base_folder.as_deref(), // Pass the base folder for proper path calculation
            ).context("Failed to determine output path")?;

            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).context("Failed to create output directories")?;
            }

            println!("Preparing to convert {:?} to {:?}", input_path, output_path);

            // Create a temporary app-like structure for the conversion tool call
            let temp_app = TempConversionContext {
                converter_tool,
                output_format,
                skeleton_file: skeleton_file.clone(),
                hkxcmd_path: hkxcmd_path.clone(),
                hkxc_path: hkxc_path.clone(),
                hkxconv_path: hkxconv_path.clone(),
                sse_to_le_hko_path: sse_to_le_hko_path.clone(),
                havok_behavior_post_process_path: havok_behavior_post_process_path.clone(),
                hct_standalone_filter_manager_path: hct_standalone_filter_manager_path.clone(),
                hct_filter_manager_dll_path: hct_filter_manager_dll_path.clone(),
            };

            // Clone needed data for the async task
            let input_path_clone = input_path.clone();
            let output_path_clone = output_path.clone();
            let progress_tx_clone = progress_tx.clone();
            let file_name = input_path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Create individual conversion task
            let conversion_task = tokio::spawn(async move {
                // Send progress update when starting this file
                let _ = progress_tx_clone.send(ConversionProgress {
                    current_file: file_name.clone(),
                    file_index: index,
                    total_files,
                    status: ConversionStatus::Running {
                        current_file: file_name.clone(),
                        progress: index,
                        total: total_files,
                    },
                });

                println!("Starting conversion of {:?}", input_path_clone);

                // Run the actual conversion
                let result = temp_app.run_conversion_tool(&input_path_clone, &output_path_clone).await;

                match result {
                    Ok(()) => {
                        if !output_path_clone.exists() {
                            let error_msg = format!("Output file was not created: {:?}", output_path_clone);
                            eprintln!("ERROR: {}", error_msg);
                            let _ = progress_tx_clone.send(ConversionProgress {
                                current_file: file_name.clone(),
                                file_index: index,
                                total_files,
                                status: ConversionStatus::Error {
                                    message: format!("Failed to convert {}", file_name),
                                },
                            });
                            return Err(anyhow::anyhow!(error_msg));
                        }

                        println!("Completed conversion of {:?}", input_path_clone);
                        let metadata = fs::metadata(&output_path_clone)?;
                        println!("Output file size: {} bytes", metadata.len());
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("ERROR converting {}: {}", file_name, e);
                        let _ = progress_tx_clone.send(ConversionProgress {
                            current_file: file_name.clone(),
                            file_index: index,
                            total_files,
                            status: ConversionStatus::Error {
                                message: format!("Failed to convert {}", file_name),
                            },
                        });
                        Err(e)
                    }
                }
            });

            conversion_tasks.push(conversion_task);
        }

        // Wait for all conversions to complete concurrently
        let results = join_all(conversion_tasks).await;
        
        // Check results and count successes
        let mut successful_conversions = 0;
        let mut failed_conversions = 0;
        for result in results {
            // Check for cancellation
            if cancel_rx.try_recv().is_ok() {
                let _ = progress_tx.send(ConversionProgress {
                    current_file: "Cancelled".to_string(),
                    file_index: successful_conversions,
                    total_files,
                    status: ConversionStatus::Error {
                        message: "Conversion cancelled".to_string(),
                    },
                });
                return Ok(());
            }

            match result {
                Ok(Ok(())) => {
                    successful_conversions += 1;
                }
                Ok(Err(e)) => {
                    eprintln!("ERROR: Conversion task failed: {}", e);
                    failed_conversions += 1;
                }
                Err(e) => {
                    eprintln!("ERROR: Task execution failed: {}", e);
                    failed_conversions += 1;
                }
            }
        }

        // Send completion message
        if failed_conversions > 0 {
            let _ = progress_tx.send(ConversionProgress {
                current_file: "Completed".to_string(),
                file_index: successful_conversions,
                total_files,
                status: ConversionStatus::Error {
                    message: format!("Converted {} of {} files ({} failed)", successful_conversions, total_files, failed_conversions),
                },
            });
        } else {
            let _ = progress_tx.send(ConversionProgress {
                current_file: "Completed".to_string(),
                file_index: successful_conversions,
                total_files,
                status: ConversionStatus::Completed {
                    message: format!("Successfully converted {} of {} files", successful_conversions, total_files),
                },
            });
        }

        Ok(())
    }

    // Static helper method for output path calculation
    fn get_output_path_static(
        input_path: &Path,
        output_folder: &Path,
        output_suffix: &str,
        output_format: OutputFormat,
        custom_extension: &Option<String>,
        base_folder: Option<&Path>,
    ) -> Option<PathBuf> {
        let file_name = input_path.file_stem()?.to_str()?;
        
        let extension = if let Some(custom_ext) = custom_extension {
            custom_ext.as_str()
        } else {
            output_format.extension()
        };

        // Calculate relative path from base folder to maintain folder structure
        let relative_path = if let Some(base_folder) = base_folder {
            // If we have a base folder, calculate relative path from it
            if let Ok(relative) = input_path.parent().unwrap_or(Path::new("")).strip_prefix(base_folder) {
                relative.to_path_buf()
            } else {
                // Fallback: use the parent directory relative to the input path
                input_path.parent().unwrap_or(Path::new("")).to_path_buf()
            }
        } else {
            // No base folder, just use the filename
            PathBuf::new()
        };

        let output_name = if output_suffix.is_empty() {
            format!("{}.{}", file_name, extension)
        } else {
            format!("{}_{}.{}", file_name, output_suffix, extension)
        };

        Some(output_folder.join(relative_path).join(output_name))
    }

    /// Get relative path for display purposes
    fn get_relative_path_display(&self, path: &Path) -> String {
        if let Some(base_folder) = &self.base_folder {
            if let Ok(relative) = path.strip_prefix(base_folder) {
                relative.to_string_lossy().to_string()
            } else {
                path.file_name().unwrap_or_default().to_string_lossy().to_string()
            }
        } else {
            path.file_name().unwrap_or_default().to_string_lossy().to_string()
        }
    }

    fn render_main_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.heading(
                RichText::new("Composite HKX Conversion Tool")
                    .size(24.0)
                    .color(Color32::LIGHT_BLUE),
            );
            ui.add_space(10.0);
        });

        ui.separator();

        egui::Grid::new("main_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Converter Tool:");
                ui.horizontal(|ui| {
                    for tool in [ConverterTool::HkxCmd, ConverterTool::Hct, ConverterTool::HavokBehaviorPostProcess, ConverterTool::HkxC, ConverterTool::HkxConv] {
                        let response = ui
                            .selectable_label(self.converter_tool == tool, tool.label());
                        
                        if response.clicked() {
                            self.converter_tool = tool;
                            // Reset input file extension if tool doesn't support current filter
                            if !tool.available_input_extensions().contains(&self.input_file_extension) {
                                self.input_file_extension = InputFileExtension::Hkx;
                            }
                            // Reset output format if tool doesn't support current format
                            let available_formats = self.available_output_formats();
                            if !available_formats.contains(&self.output_format) {
                                if !available_formats.is_empty() {
                                    self.output_format = available_formats[0];
                                }
                            }
                        }
                        
                        // Show tooltip on hover
                        if response.hovered() {
                            if let Some(hover_pos) = response.hover_pos() {
                                self.show_tool_tooltip(ui, tool, hover_pos);
                            }
                        }
                    }
                });
                ui.end_row();

                ui.label("Input File Filter:");
                ui.horizontal(|ui| {
                    let available_filters = self.converter_tool.available_input_extensions();
                    
                    for filter in available_filters {
                        if ui
                            .selectable_label(self.input_file_extension == filter, filter.label_for_tool(self.converter_tool))
                            .clicked()
                        {
                            self.input_file_extension = filter;
                        }
                    }
                    
                    // Reset to a valid filter if current selection is not available
                    if (self.converter_tool == ConverterTool::HkxC || self.converter_tool == ConverterTool::HkxConv) && self.input_file_extension == InputFileExtension::Kf {
                        self.input_file_extension = InputFileExtension::Hkx;
                    }
                });
                ui.end_row();

                ui.label("Input Files:");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Browse Files").clicked() {
                            if let Some(paths) = FileDialog::new().pick_files() {
                                self.input_paths = paths;
                                // Clear base folder for individual file selection
                                self.base_folder = None;
                                self.update_output_folder();
                            }
                        }
                        if ui.button("Select Folder").clicked() {
                            if let Some(folder) = FileDialog::new().pick_folder() {
                                if let Err(e) = self.add_files_from_folder(&folder, false) {
                                    eprintln!("Error adding files from folder: {}", e);
                                }
                                self.update_output_folder();
                            }
                        }
                        if ui.button("Select Folder (+ Subfolders)").clicked() {
                            if let Some(folder) = FileDialog::new().pick_folder() {
                                if let Err(e) = self.add_files_from_folder(&folder, true) {
                                    eprintln!("Error adding files from folders: {}", e);
                                }
                                self.update_output_folder();
                            }
                        }
                    });
                });
                ui.end_row();

                // Skeleton file selection (only show for KF conversion)
                if self.output_format.requires_skeleton() {
                    ui.label("Skeleton File:");
                    ui.horizontal(|ui| {
                        if let Some(ref skeleton_file) = self.skeleton_file {
                            ui.label(skeleton_file.file_name().unwrap_or_default().to_string_lossy());
                        } 
                        // else {
                        //     ui.label("(required for animation conversion)");
                        // }
                        if ui.button("Browse").clicked() {
                            if let Some(file) = FileDialog::new()
                                .add_filter("HKX files", &["hkx"])
                                .pick_file()
                            {
                                self.skeleton_file = Some(file);
                            }
                        }
                        if self.skeleton_file.is_some() && ui.button("Clear").clicked() {
                            self.skeleton_file = None;
                        }
                    });
                    ui.end_row();
                }

                ui.label("Output Folder:");
                self.render_output_folder(ui);
                ui.end_row();

                ui.label("Output Suffix:");
                ui.text_edit_singleline(&mut self.output_suffix);
                ui.end_row();

                ui.label("Custom Extension:");
                ui.horizontal(|ui| {
                    let mut extension_text = self.custom_extension.as_ref().cloned().unwrap_or_default();
                    if ui.text_edit_singleline(&mut extension_text).changed() {
                        self.custom_extension = if extension_text.is_empty() {
                            None
                        } else {
                            Some(extension_text)
                        };
                    }
                    // ui.label("(optional - leave empty to use format default)");
                });
                ui.end_row();

                ui.label("Output Format:");
                self.render_output_format(ui);
                ui.end_row();
            });

        ui.add_space(10.0);

        // Selected Files section outside the grid for more space
        ui.horizontal(|ui| {
            ui.label("Selected Files:");
            ui.label(format!("{} files selected", self.input_paths.len()));
            if ui.button("Clear All").clicked() {
                self.input_paths.clear();
                self.base_folder = None;
                // Reset the manually set flag when clearing all files
                self.output_folder_manually_set = false;
            }
        });
        
        // Show base folder information if set
        if let Some(ref base_folder) = self.base_folder {
            ui.horizontal(|ui| {
                ui.label(RichText::new("📁 Base folder:").color(Color32::from_rgb(100, 150, 200)).size(12.0));
                ui.label(RichText::new(base_folder.to_string_lossy()).color(Color32::from_rgb(150, 150, 150)).size(12.0));
            });
        }
        
        // Show drag and drop hint
        ui.horizontal(|ui| {
            ui.label(RichText::new("💡 Tip: You can drag and drop files or folders directly onto this window").color(Color32::from_rgb(100, 100, 100)).size(12.0));
        });
        
        // Show HCT processing note
        // if self.converter_tool == ConverterTool::Hct {
        //     ui.horizontal(|ui| {
        //         ui.label(RichText::new("ℹ️ HCT files use isolated temp directories for safe concurrent processing").color(Color32::from_rgb(100, 100, 100)).size(12.0));
        //     });
        // }
        
        // Scrollable area for file list - takes remaining available space
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let mut files_to_remove = Vec::new();
                for (index, path) in self.input_paths.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("❌").clicked() {
                            files_to_remove.push(index);
                        }
                        ui.label(self.get_relative_path_display(path));
                    });
                }
                
                // Remove files after iteration
                for index in files_to_remove.iter().rev() {
                    self.input_paths.remove(*index);
                }
            });
    }

    fn render_output_folder(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            if let Some(ref output_folder) = self.output_folder {
                ui.label(output_folder.to_string_lossy());
                // Show indicator if manually set
                if self.output_folder_manually_set {
                    ui.label(RichText::new("🔒").color(Color32::from_rgb(100, 150, 200)).size(12.0));
                }
            }
            
            ui.horizontal(|ui| {
                if ui.button("Browse").clicked() {
                    if let Some(folder) = FileDialog::new().pick_folder() {
                        self.output_folder = Some(folder);
                        self.output_folder_manually_set = true;
                    }
                }
                
                // Add "Open Folder" button
                if let Some(ref output_folder) = self.output_folder {
                    if ui.button("Open Folder").clicked() {
                        Self::open_folder_in_explorer(output_folder);
                    }
                }
            });
        });
    }

    fn render_output_format(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let available_formats = self.available_output_formats();
            
            for format in available_formats {
                if ui
                    .selectable_label(self.output_format == format, format.label())
                    .clicked()
                {
                    self.output_format = format;
                }
            }
            
            // Reset to a valid format if current selection is not available
            let available_formats = self.available_output_formats();
            if !available_formats.contains(&self.output_format) {
                if !available_formats.is_empty() {
                    self.output_format = available_formats[0];
                }
            }
            
            // Reset to a valid filter if current selection is not available
            if !self.converter_tool.available_input_extensions().contains(&self.input_file_extension) {
                self.input_file_extension = InputFileExtension::Hkx;
            }
        });
    }

    fn handle_conversion(&mut self, ui: &mut Ui) {
        // Check for progress updates
        if let Some(progress_rx) = &mut self.progress_rx {
            while let Ok(progress) = progress_rx.try_recv() {
                self.conversion_status = progress.status;
                // Request repaint to update UI immediately
                ui.ctx().request_repaint();
            }
        }

        // Clone the current status to avoid borrow checker issues
        let current_status = self.conversion_status.clone();
        
        // Display status messages if running, completed, or error
        match &current_status {
            ConversionStatus::Running { current_file, progress, total } => {
                ui.add_space(20.0);

                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(format!("Converting: {}", current_file))
                            .size(14.0)
                            .color(Color32::from_rgb(100, 150, 255))
                    );
                    
                    // Progress bar
                    let progress_fraction = if *total > 0 { *progress as f32 / *total as f32 } else { 0.0 };
                    let progress_bar = egui::ProgressBar::new(progress_fraction)
                        .text(format!("{}/{}", progress, total))
                        .desired_height(20.0);
                    ui.add(progress_bar);
                });
                
                // Request continuous repaints while running
                ui.ctx().request_repaint();
            }
            ConversionStatus::Completed { message } => {
                ui.add_space(20.0);

                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(message)
                            .size(14.0)
                            .color(Color32::from_rgb(100, 200, 100))
                            .strong()
                    );
                });
            }
            ConversionStatus::Error { message } => {
                ui.add_space(20.0);

                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(message)
                            .size(14.0)
                            .color(Color32::from_rgb(255, 120, 120))
                            .strong()
                    );
                });
            }
            ConversionStatus::Idle => {
                // No status message when idle
            }
        }
                
        // Big prominent button at the bottom
        ui.vertical_centered(|ui| {
            match current_status {
                ConversionStatus::Idle | ConversionStatus::Completed { .. } | ConversionStatus::Error { .. } => {
                    if matches!(current_status, ConversionStatus::Idle) {
                        ui.add_space(20.0);
                    }

                    let button = egui::Button::new(
                        RichText::new("🚀 RUN CONVERSION")
                            .size(18.0)
                            .strong()
                    )
                    .min_size(egui::Vec2::new(ui.available_width() - 20.0, 50.0))
                    .fill(Color32::from_rgb(70, 130, 220));
                    
                    if ui.add(button).clicked() {
                        // Reset status before starting new conversion
                        self.conversion_status = ConversionStatus::Idle;
                        self.progress_rx = None;
                        self.cancel_tx = None;
                        self.start_conversion();
                    }
                }
                ConversionStatus::Running { .. } => {
                    let button = egui::Button::new(
                        RichText::new("⏹ CANCEL CONVERSION")
                            .size(16.0)
                            .strong()
                    )
                    .min_size(egui::Vec2::new(ui.available_width() - 20.0, 45.0))
                    .fill(Color32::from_rgb(200, 80, 80));
                    
                    if ui.add(button).clicked() {
                        if let Some(cancel_tx) = self.cancel_tx.take() {
                            let _ = cancel_tx.send(());
                        }
                        self.conversion_status = ConversionStatus::Idle;
                    }
                }
            }
        });
        
        ui.add_space(20.0);
    }
}

impl eframe::App for HkxToolsApp {
    fn update(&mut self, ctx: &EguiContext, _frame: &mut Frame) {
        // Check if files are being hovered over the window
        let files_being_hovered = ctx.input(|i| i.raw.hovered_files.len() > 0);
        let hovered_files_count = ctx.input(|i| i.raw.hovered_files.len());

        // Handle drag and drop files
        if !ctx.input(|i| i.raw.dropped_files.is_empty()) {
            let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
            self.handle_dropped_files(dropped_files);
        }

        // Bottom panel for conversion button (always at bottom)
        egui::TopBottomPanel::bottom("conversion_panel")
            .resizable(false)
            .show(ctx, |ui| {
                self.handle_conversion(ui);
            });

        // Main content in the center
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_ui(ui);
        });

        // Show drag and drop overlay when files are being hovered
        if files_being_hovered {
            self.render_drag_drop_overlay(ctx, hovered_files_count);
        }
    }
}



#[tokio::main]
async fn main() -> Result<(), eframe::Error> {
    // Create a tokio runtime handle for the GUI
    let tokio_handle = tokio::runtime::Handle::current();

    // Write hkxcmd.exe, hkxc.exe, hkxconv.exe, and HCT .hko file to a temporary location
    let temp_dir = tempfile::Builder::new()
        .prefix("hkxtools_")
        .tempdir()
        .unwrap();
    
    let hkxcmd_path = temp_dir.path().join("hkxcmd.exe");
    let hkxc_path = temp_dir.path().join("hkxc.exe");
    let hkxconv_path = temp_dir.path().join("hkxconv.exe");
    let sse_to_le_hko_path = temp_dir.path().join("_SSEtoLE.hko");
    let havok_behavior_post_process_path = temp_dir.path().join("HavokBehaviorPostProcess.exe");
    let hct_standalone_filter_manager_path = temp_dir.path().join("hctStandAloneFilterManager.exe");
    let hct_filter_manager_dll_path = temp_dir.path().join("hctFilterManager.dll");
    
    fs::write(&hkxcmd_path, HKXCMD_EXE).unwrap();
    fs::write(&hkxc_path, HKXC_EXE).unwrap();
    fs::write(&hkxconv_path, HKXCONV_EXE).unwrap();
    fs::write(&sse_to_le_hko_path, SSE_TO_LE_HKO).unwrap();
    fs::write(&havok_behavior_post_process_path, HAVOK_BEHAVIOR_POST_PROCESS_EXE).unwrap();
    fs::write(&hct_standalone_filter_manager_path, HCT_STANDALONE_FILTER_MANAGER_EXE).unwrap();
    fs::write(&hct_filter_manager_dll_path, HCT_FILTER_MANAGER_DLL).unwrap();

    println!("Extracted hkxcmd.exe to: {:?}", hkxcmd_path);
    println!("Extracted hkxc.exe to: {:?}", hkxc_path);
    println!("Extracted hkxconv.exe to: {:?}", hkxconv_path);
    println!("Extracted _SSEtoLE.hko to: {:?}", sse_to_le_hko_path);
    println!("Extracted HavokBehaviorPostProcess.exe to: {:?}", havok_behavior_post_process_path);
    println!("Extracted hctStandAloneFilterManager.exe to: {:?}", hct_standalone_filter_manager_path);
    println!("Extracted hctFilterManager.dll to: {:?}", hct_filter_manager_dll_path);

    // Window width and height
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 600.0]),
        ..Default::default()
    };
    
    // Keep temp_dir alive for the entire application lifetime
    let _temp_dir_guard = temp_dir;
    
    eframe::run_native(
        "Composite HKX Conversion GUI",
        options,
        Box::new(move |_cc| Ok(Box::new(HkxToolsApp::new(hkxcmd_path, hkxc_path, hkxconv_path, sse_to_le_hko_path, havok_behavior_post_process_path, hct_standalone_filter_manager_path, hct_filter_manager_dll_path, tokio_handle)))),
    )
}
