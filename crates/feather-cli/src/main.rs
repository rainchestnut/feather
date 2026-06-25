//! Command-line entry point for Feather CAD lightweight conversion.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use feather_lite::{
    BatchConversionOptions, BatchItem, BatchItemStatus, ConversionOptions, ImportLimits,
    ImportOptions, InspectOptions, JobConversionSettings, JobImportLimits, JobRecord,
    JobReferencePathMapping, JobResult, JobStatus, LocalJobStore, ReferencePathMapping,
    convert_path_to_glb, dump_embedded_visual_assets_with_limits, format_capabilities,
    format_capabilities_json, inspect_path, run_batch_conversion,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    let command = args.remove(0);
    match command.as_str() {
        "inspect" => inspect(&args),
        "convert" => convert(&args),
        "batch" => batch(&args),
        "job" => job(&args),
        "dump-cache" => dump_cache(&args),
        "formats" => formats(&args),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("unknown command `{other}`")),
    }
}

fn inspect(args: &[String]) -> Result<(), String> {
    let mut input = None::<PathBuf>;
    let mut json = false;
    let mut check_import = false;
    let mut resolve_dirs = Vec::<PathBuf>::new();
    let mut reference_path_mappings = Vec::<ReferencePathMapping>::new();
    let mut limits = ImportLimits::default();
    let mut max_lod_error = 0.0_f32;

    let mut index = 0;
    while index < args.len() {
        if parse_import_limit_option(args, &mut index, &mut limits)? {
            index += 1;
            continue;
        }
        if parse_reference_option(
            args,
            &mut index,
            &mut resolve_dirs,
            &mut reference_path_mappings,
        )? {
            index += 1;
            continue;
        }
        match args[index].as_str() {
            "--json" => {
                json = true;
            }
            "--check" => {
                check_import = true;
            }
            "--chord-error" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or("--chord-error requires a positive source-unit value")?;
                max_lod_error = parse_positive_f32("--chord-error", value)?;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown inspect option `{value}`"));
            }
            value => {
                if input.is_some() {
                    return Err("inspect accepts only one input path".to_string());
                }
                input = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }

    let import_options = ImportOptions {
        resolve_dirs,
        reference_path_mappings,
        max_lod_error,
        limits,
        ..ImportOptions::default()
    };
    let report = inspect_path(
        &input.ok_or("inspect requires an input path")?,
        &InspectOptions {
            import: import_options,
            check_import,
        },
    )
    .map_err(|error| error.to_string())?;

    if json {
        print!("{}", report.to_json_string());
        return Ok(());
    }

    let path = report
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    println!("path: {path}");
    println!("format: {}", report.probe.format.label());
    println!("confidence: {:?}", report.probe.confidence);
    println!("embedded_cache: {}", report.probe.has_embedded_cache);
    println!("reason: {}", report.probe.reason);
    if let Some(container_kind) = report.probe.container_kind {
        println!("container_kind: {container_kind}");
    }
    if let Some(source_version) = &report.probe.source_version {
        println!("source_version: {source_version}");
    }
    if let Some(native_visualization) = report.probe.native_visualization {
        println!("native_visualization: {native_visualization}");
    }
    if let Some(coarse_format) = report.coarse_format {
        println!("coarse_format: {}", coarse_format.label());
    }
    if let Some(capability) = report.capability() {
        println!("capability_status: {}", capability.status);
        println!(
            "requires_visual_payload: {}",
            capability.requires_visual_payload
        );
        println!(
            "supports_external_references: {}",
            capability.supports_external_references
        );
        println!(
            "native_brep_tessellation: {}",
            capability.native_brep_tessellation
        );
    }
    println!("visual_assets: {}", report.visual_assets.len());
    if let Some(import_check) = &report.import_check {
        println!("importable: {}", import_check.importable);
        if import_check.importable {
            println!("meshes: {}", import_check.mesh_count.unwrap_or(0));
            println!("triangles: {}", import_check.triangle_count.unwrap_or(0));
        } else if let Some(error) = &import_check.error {
            if let Some(category) = import_check.failure_category {
                println!("failure_category: {category}");
            }
            if let Some(condition) = import_check.required_condition {
                println!("required_condition: {condition}");
            }
            println!("import_error: {error}");
        }
    }
    for (asset_index, asset) in report.visual_assets.iter().enumerate() {
        let name = asset.name.as_deref().unwrap_or("-");
        println!(
            "asset: {asset_index}\tkind: {}\tsource: {}\trange: {}..{}\tname: {name}",
            asset.kind.label(),
            asset.source.label(),
            asset.byte_start,
            asset.byte_end,
        );
    }
    Ok(())
}

fn formats(args: &[String]) -> Result<(), String> {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown formats option `{value}`"));
            }
            value => {
                return Err(format!("formats does not accept input path `{value}`"));
            }
        }
    }

    if json {
        print!("{}", format_capabilities_json());
        return Ok(());
    }

    println!("format\textensions\tstatus\tconversion_path\tlimitation");
    for capability in format_capabilities() {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            capability.format.label(),
            capability.extensions.join(","),
            capability.status,
            capability.conversion_path,
            capability.limitation
        );
    }
    Ok(())
}

fn convert(args: &[String]) -> Result<(), String> {
    let mut input = None::<PathBuf>;
    let mut output = None::<PathBuf>;
    let mut metadata_path = None::<PathBuf>;
    let mut write_metadata = true;
    let mut conversion = CliConversionSettings::default();

    let mut index = 0;
    while index < args.len() {
        if parse_conversion_option(args, &mut index, &mut conversion)? {
            index += 1;
            continue;
        }
        match args[index].as_str() {
            "-o" | "--output" => {
                index += 1;
                output = Some(PathBuf::from(
                    args.get(index).ok_or("--output requires a path")?,
                ));
            }
            "--metadata" => {
                index += 1;
                metadata_path = Some(PathBuf::from(
                    args.get(index).ok_or("--metadata requires a path")?,
                ));
                write_metadata = true;
            }
            "--no-metadata" => {
                write_metadata = false;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown convert option `{value}`"));
            }
            value => {
                if input.is_some() {
                    return Err("convert accepts only one input path".to_string());
                }
                input = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }

    let input = input.ok_or("convert requires an input path")?;
    let output = output.ok_or("convert requires -o/--output")?;
    let options = build_conversion_options(&conversion, write_metadata, metadata_path);

    let summary = convert_path_to_glb(&input, &output, &options)
        .map_err(|error| format!("conversion failed: {error}"))?;

    println!("format: {}", summary.source_format);
    println!("output: {}", summary.output_path.display());
    if let Some(metadata_path) = summary.metadata_path {
        println!("metadata: {}", metadata_path.display());
    }
    println!("nodes: {}", summary.node_count);
    println!("meshes: {}", summary.mesh_count);
    println!("primitives: {}", summary.primitive_count);
    println!("vertices: {}", summary.vertex_count);
    println!("triangles: {}", summary.triangle_count);
    Ok(())
}

fn batch(args: &[String]) -> Result<(), String> {
    let mut inputs = Vec::<PathBuf>::new();
    let mut output_dir = None::<PathBuf>;
    let mut manifest_path = None::<PathBuf>;
    let mut check_only = false;
    let mut conversion = CliConversionSettings::default();
    let mut write_metadata = true;

    let mut index = 0;
    while index < args.len() {
        if parse_conversion_option(args, &mut index, &mut conversion)? {
            index += 1;
            continue;
        }
        match args[index].as_str() {
            "--out" => {
                index += 1;
                output_dir = Some(PathBuf::from(
                    args.get(index).ok_or("--out requires a path")?,
                ));
            }
            "--manifest" => {
                index += 1;
                manifest_path = Some(PathBuf::from(
                    args.get(index).ok_or("--manifest requires a path")?,
                ));
            }
            "--no-metadata" => {
                write_metadata = false;
            }
            "--check-only" => {
                check_only = true;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown batch option `{value}`"));
            }
            value => inputs.push(PathBuf::from(value)),
        }
        index += 1;
    }

    if inputs.is_empty() {
        return Err("batch requires at least one input path".to_string());
    }
    let output_dir = output_dir.ok_or("batch requires --out <directory>")?;
    let run = run_batch_conversion(
        &inputs,
        &BatchConversionOptions {
            output_dir,
            manifest_path,
            check_only,
            conversion: build_conversion_options(&conversion, write_metadata, None),
        },
    )
    .map_err(|error| error.to_string())?;

    for item in &run.report.items {
        print_batch_item(item);
    }

    let failed_count = run.report.failed_count();
    println!("inputs: {}", run.report.input_count());
    println!("converted: {}", run.report.converted_count());
    if check_only || run.report.checked_count() > 0 {
        println!("checked: {}", run.report.checked_count());
    }
    println!("failed: {failed_count}");
    println!("manifest: {}", run.manifest_path.display());

    if failed_count > 0 {
        return Err(format!(
            "batch completed with {failed_count} failed conversions; manifest: {}",
            run.manifest_path.display()
        ));
    }
    Ok(())
}

fn job(args: &[String]) -> Result<(), String> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err("job requires convert, batch, status, or retry".to_string());
    };
    match subcommand.as_str() {
        "convert" => job_convert(rest),
        "batch" => job_batch(rest),
        "status" => job_status(rest),
        "retry" => job_retry(rest),
        other => Err(format!("unknown job subcommand `{other}`")),
    }
}

fn job_convert(args: &[String]) -> Result<(), String> {
    let mut input = None::<PathBuf>;
    let mut store_dir = PathBuf::from(".feather-jobs");
    let mut json = false;
    let mut write_metadata = true;
    let mut conversion = CliConversionSettings::default();

    let mut index = 0;
    while index < args.len() {
        if parse_conversion_option(args, &mut index, &mut conversion)? {
            index += 1;
            continue;
        }
        match args[index].as_str() {
            "--store" => {
                index += 1;
                store_dir = PathBuf::from(args.get(index).ok_or("--store requires a directory")?);
            }
            "--json" => {
                json = true;
            }
            "--no-metadata" => {
                write_metadata = false;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown job convert option `{value}`"));
            }
            value => {
                if input.is_some() {
                    return Err("job convert accepts only one input path".to_string());
                }
                input = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }

    let store = LocalJobStore::new(store_dir);
    let job = store
        .create_conversion_job(
            input.ok_or("job convert requires an input path")?,
            build_job_conversion_settings(&conversion, write_metadata),
        )
        .map_err(|error| error.to_string())?;
    let job = store
        .run_job(&job.job_id)
        .map_err(|error| error.to_string())?;
    print_job_record(&job, json);
    fail_if_job_failed(&job)
}

fn job_batch(args: &[String]) -> Result<(), String> {
    let mut inputs = Vec::<PathBuf>::new();
    let mut store_dir = PathBuf::from(".feather-jobs");
    let mut json = false;
    let mut check_only = false;
    let mut write_metadata = true;
    let mut conversion = CliConversionSettings::default();

    let mut index = 0;
    while index < args.len() {
        if parse_conversion_option(args, &mut index, &mut conversion)? {
            index += 1;
            continue;
        }
        match args[index].as_str() {
            "--store" => {
                index += 1;
                store_dir = PathBuf::from(args.get(index).ok_or("--store requires a directory")?);
            }
            "--json" => {
                json = true;
            }
            "--check-only" => {
                check_only = true;
            }
            "--no-metadata" => {
                write_metadata = false;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown job batch option `{value}`"));
            }
            value => inputs.push(PathBuf::from(value)),
        }
        index += 1;
    }

    if inputs.is_empty() {
        return Err("job batch requires at least one input path".to_string());
    }

    let store = LocalJobStore::new(store_dir);
    let job = store
        .create_batch_job(
            inputs,
            check_only,
            build_job_conversion_settings(&conversion, write_metadata),
        )
        .map_err(|error| error.to_string())?;
    let job = store
        .run_job(&job.job_id)
        .map_err(|error| error.to_string())?;
    print_job_record(&job, json);
    fail_if_job_failed(&job)
}

fn job_status(args: &[String]) -> Result<(), String> {
    let mut job_id = None::<String>;
    let mut store_dir = PathBuf::from(".feather-jobs");
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--store" => {
                index += 1;
                store_dir = PathBuf::from(args.get(index).ok_or("--store requires a directory")?);
            }
            "--json" => {
                json = true;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown job status option `{value}`"));
            }
            value => {
                if job_id.is_some() {
                    return Err("job status accepts only one job id".to_string());
                }
                job_id = Some(value.to_string());
            }
        }
        index += 1;
    }

    let store = LocalJobStore::new(store_dir);
    let job = store
        .load_job(&job_id.ok_or("job status requires a job id")?)
        .map_err(|error| error.to_string())?;
    print_job_record(&job, json);
    Ok(())
}

fn job_retry(args: &[String]) -> Result<(), String> {
    let mut job_id = None::<String>;
    let mut store_dir = PathBuf::from(".feather-jobs");
    let mut json = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--store" => {
                index += 1;
                store_dir = PathBuf::from(args.get(index).ok_or("--store requires a directory")?);
            }
            "--json" => {
                json = true;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown job retry option `{value}`"));
            }
            value => {
                if job_id.is_some() {
                    return Err("job retry accepts only one job id".to_string());
                }
                job_id = Some(value.to_string());
            }
        }
        index += 1;
    }

    let store = LocalJobStore::new(store_dir);
    let job = store
        .retry_job(&job_id.ok_or("job retry requires a job id")?)
        .map_err(|error| error.to_string())?;
    print_job_record(&job, json);
    fail_if_job_failed(&job)
}

fn print_job_record(job: &JobRecord, json: bool) {
    if json {
        print!("{}", job.to_json_string());
        return;
    }

    println!("job: {}", job.job_id);
    println!("status: {}", job.status.as_str());
    println!("stage: {}", job.stage.as_str());
    println!("artifacts: {}", job.artifacts.root_dir.display());
    if let Some(result) = &job.result {
        match result {
            JobResult::Conversion {
                output_path,
                metadata_path,
                triangle_count,
                ..
            } => {
                println!("output: {}", output_path.display());
                if let Some(metadata_path) = metadata_path {
                    println!("metadata: {}", metadata_path.display());
                }
                println!("triangles: {triangle_count}");
            }
            JobResult::Batch {
                manifest_path,
                input_count,
                converted_count,
                checked_count,
                failed_count,
            } => {
                println!("manifest: {}", manifest_path.display());
                println!("inputs: {input_count}");
                println!("converted: {converted_count}");
                println!("checked: {checked_count}");
                println!("failed: {failed_count}");
            }
        }
    }
    if let Some(failure) = &job.failure {
        println!("failure_stage: {}", failure.stage);
        println!("failure_category: {}", failure.category);
        println!("retryable: {}", failure.retryable);
        println!("failure: {}", failure.message);
    }
}

fn fail_if_job_failed(job: &JobRecord) -> Result<(), String> {
    if job.status == JobStatus::Failed {
        let message = job
            .failure
            .as_ref()
            .map(|failure| failure.message.clone())
            .unwrap_or_else(|| "job failed".to_string());
        return Err(format!("job `{}` failed: {message}", job.job_id));
    }
    Ok(())
}

fn print_batch_item(item: &BatchItem) {
    match &item.status {
        BatchItemStatus::Ok {
            output_path,
            triangle_count,
            ..
        } => {
            println!(
                "ok\t{}\t{}\t{} triangles",
                item.input_path, output_path, triangle_count
            );
        }
        BatchItemStatus::Checked { triangle_count, .. } => {
            println!("checked\t{}\t{} triangles", item.input_path, triangle_count);
        }
        BatchItemStatus::Error { message, .. } => {
            println!("error\t{}\t{}", item.input_path, message);
        }
    }
}

fn dump_cache(args: &[String]) -> Result<(), String> {
    let mut input = None::<PathBuf>;
    let mut output_dir = None::<PathBuf>;
    let mut limits = ImportLimits::default();

    let mut index = 0;
    while index < args.len() {
        if parse_import_limit_option(args, &mut index, &mut limits)? {
            index += 1;
            continue;
        }
        match args[index].as_str() {
            "--out" => {
                index += 1;
                output_dir = Some(PathBuf::from(
                    args.get(index).ok_or("--out requires a path")?,
                ));
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown dump-cache option `{value}`"));
            }
            value => {
                if input.is_some() {
                    return Err("dump-cache accepts only one input path".to_string());
                }
                input = Some(PathBuf::from(value));
            }
        }
        index += 1;
    }

    let input = input.ok_or("dump-cache requires an input path")?;
    let output_dir = output_dir.ok_or("dump-cache requires --out <dir>")?;
    let report = dump_embedded_visual_assets_with_limits(&input, &output_dir, &limits)
        .map_err(|error| error.to_string())?;

    println!("assets: {}", report.asset_count());
    println!("manifest: {}", report.manifest_path.display());
    for asset in &report.assets {
        println!(
            "{}\t{}\t{}\t{}..{}\t{}",
            asset.index,
            asset.kind,
            asset.source,
            asset.byte_start,
            asset.byte_end,
            asset.file_name
        );
    }
    Ok(())
}

fn print_usage() {
    println!("Feather CAD lightweight conversion");
    println!();
    println!("Commands:");
    println!("  feather formats [--json]");
    println!(
        "  feather inspect <input> [--json] [--check] [--chord-error <source-unit-value>] [--resolve-dir <dir>] [--map-root <old-prefix>=<new-root>] [--max-input-bytes <bytes>] [--max-ole-streams <count>] [--max-ole-stream-bytes <bytes>] [--max-ole-total-bytes <bytes>] [--max-archive-entries <count>] [--max-archive-entry-bytes <bytes>] [--max-archive-total-bytes <bytes>] [--max-step-curve-segments <count>] [--max-step-spline-degree <count>] [--max-step-spline-control-points <count>] [--max-step-face-loops <count>] [--max-step-face-vertices <count>] [--max-step-assembly-nodes <count>]"
    );
    println!(
        "  feather convert <input> -o <output.glb> [--metadata <path>] [--chord-error <source-unit-value>] [--resolve-dir <dir>] [--map-root <old-prefix>=<new-root>] [--max-input-bytes <bytes>] [--max-ole-streams <count>] [--max-ole-stream-bytes <bytes>] [--max-ole-total-bytes <bytes>] [--max-archive-entries <count>] [--max-archive-entry-bytes <bytes>] [--max-archive-total-bytes <bytes>] [--max-step-curve-segments <count>] [--max-step-spline-degree <count>] [--max-step-spline-control-points <count>] [--max-step-face-loops <count>] [--max-step-face-vertices <count>] [--max-step-assembly-nodes <count>] [--lod low|medium|high|none] [--max-triangles <count>] [--quantize <grid-step>] [--no-normals]"
    );
    println!(
        "  feather batch <input...> --out <directory> [--manifest <path>] [--chord-error <source-unit-value>] [--resolve-dir <dir>] [--map-root <old-prefix>=<new-root>] [--max-input-bytes <bytes>] [--max-ole-streams <count>] [--max-ole-stream-bytes <bytes>] [--max-ole-total-bytes <bytes>] [--max-archive-entries <count>] [--max-archive-entry-bytes <bytes>] [--max-archive-total-bytes <bytes>] [--max-step-curve-segments <count>] [--max-step-spline-degree <count>] [--max-step-spline-control-points <count>] [--max-step-face-loops <count>] [--max-step-face-vertices <count>] [--max-step-assembly-nodes <count>] [--check-only] [--lod low|medium|high|none] [--max-triangles <count>] [--quantize <grid-step>] [--no-normals]"
    );
    println!("  feather job convert <input> [--store <directory>] [--json] [conversion options]");
    println!(
        "  feather job batch <input...> [--store <directory>] [--json] [--check-only] [conversion options]"
    );
    println!("  feather job status <job-id> [--store <directory>] [--json]");
    println!("  feather job retry <job-id> [--store <directory>] [--json]");
    println!(
        "  feather dump-cache <input> --out <directory> [--max-input-bytes <bytes>] [--max-ole-streams <count>] [--max-ole-stream-bytes <bytes>] [--max-ole-total-bytes <bytes>] [--max-archive-entries <count>] [--max-archive-entry-bytes <bytes>] [--max-archive-total-bytes <bytes>]"
    );
}

fn parse_positive_u64(option: &str, value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{option} expects a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{option} expects a value greater than zero"));
    }
    Ok(parsed)
}

fn parse_positive_f32(option: &str, value: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("{option} expects a positive number"))?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(format!("{option} expects a finite value greater than zero"));
    }
    Ok(parsed)
}

fn lod_triangle_budget(value: &str) -> Result<Option<u64>, String> {
    match value {
        "low" => Ok(Some(50_000)),
        "medium" => Ok(Some(150_000)),
        "high" => Ok(Some(500_000)),
        "none" => Ok(None),
        _ => Err("--lod expects low, medium, high, or none".to_string()),
    }
}

fn parse_reference_path_mapping(value: &str) -> Result<ReferencePathMapping, String> {
    let (from, to) = value
        .split_once('=')
        .ok_or("--map-root expects <old-prefix>=<new-root>")?;
    if from.trim().is_empty() {
        return Err("--map-root old prefix must not be empty".to_string());
    }
    if to.trim().is_empty() {
        return Err("--map-root new root must not be empty".to_string());
    }
    Ok(ReferencePathMapping::new(
        from.trim().to_string(),
        PathBuf::from(to.trim()),
    ))
}

/// Parses shared assembly reference options used by inspect, convert, and batch.
fn parse_reference_option(
    args: &[String],
    index: &mut usize,
    resolve_dirs: &mut Vec<PathBuf>,
    reference_path_mappings: &mut Vec<ReferencePathMapping>,
) -> Result<bool, String> {
    match args[*index].as_str() {
        "--resolve-dir" => {
            *index += 1;
            let value = args.get(*index).ok_or("--resolve-dir requires a path")?;
            resolve_dirs.push(PathBuf::from(value));
            Ok(true)
        }
        "--map-root" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or("--map-root requires <old-prefix>=<new-root>")?;
            reference_path_mappings.push(parse_reference_path_mapping(value)?);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Parsed conversion flags shared by convert and batch commands.
#[derive(Default)]
struct CliConversionSettings {
    no_optimize: bool,
    omit_normals: bool,
    max_triangles: Option<u64>,
    quantize_step: Option<f32>,
    max_lod_error: f32,
    resolve_dirs: Vec<PathBuf>,
    reference_path_mappings: Vec<ReferencePathMapping>,
    limits: ImportLimits,
}

/// Parses shared conversion options used by convert and batch.
fn parse_conversion_option(
    args: &[String],
    index: &mut usize,
    settings: &mut CliConversionSettings,
) -> Result<bool, String> {
    if parse_import_limit_option(args, index, &mut settings.limits)? {
        return Ok(true);
    }
    if parse_reference_option(
        args,
        index,
        &mut settings.resolve_dirs,
        &mut settings.reference_path_mappings,
    )? {
        return Ok(true);
    }

    match args[*index].as_str() {
        "--no-optimize" => {
            settings.no_optimize = true;
            Ok(true)
        }
        "--no-normals" => {
            settings.omit_normals = true;
            Ok(true)
        }
        "--max-triangles" => {
            *index += 1;
            let value = args.get(*index).ok_or("--max-triangles requires a count")?;
            settings.max_triangles = Some(parse_positive_u64("--max-triangles", value)?);
            Ok(true)
        }
        "--quantize" => {
            *index += 1;
            let value = args.get(*index).ok_or("--quantize requires a grid step")?;
            settings.quantize_step = Some(parse_positive_f32("--quantize", value)?);
            Ok(true)
        }
        "--chord-error" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or("--chord-error requires a positive source-unit value")?;
            settings.max_lod_error = parse_positive_f32("--chord-error", value)?;
            Ok(true)
        }
        "--lod" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or("--lod requires low, medium, high, or none")?;
            settings.max_triangles = lod_triangle_budget(value)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Parses shared input and container limits used by every file-processing
/// command.
fn parse_import_limit_option(
    args: &[String],
    index: &mut usize,
    limits: &mut ImportLimits,
) -> Result<bool, String> {
    let (option, target) = match args[*index].as_str() {
        "--max-input-bytes" => ("--max-input-bytes", &mut limits.max_input_bytes),
        "--max-ole-streams" => ("--max-ole-streams", &mut limits.max_ole_streams),
        "--max-ole-stream-bytes" => ("--max-ole-stream-bytes", &mut limits.max_ole_stream_bytes),
        "--max-ole-total-bytes" => (
            "--max-ole-total-bytes",
            &mut limits.max_ole_total_stream_bytes,
        ),
        "--max-archive-entries" => ("--max-archive-entries", &mut limits.max_archive_entries),
        "--max-archive-entry-bytes" => (
            "--max-archive-entry-bytes",
            &mut limits.max_archive_entry_uncompressed_bytes,
        ),
        "--max-archive-total-bytes" => (
            "--max-archive-total-bytes",
            &mut limits.max_archive_total_uncompressed_bytes,
        ),
        "--max-step-curve-segments" => (
            "--max-step-curve-segments",
            &mut limits.max_step_curve_segments,
        ),
        "--max-step-spline-degree" => (
            "--max-step-spline-degree",
            &mut limits.max_step_spline_degree,
        ),
        "--max-step-spline-control-points" => (
            "--max-step-spline-control-points",
            &mut limits.max_step_spline_control_points,
        ),
        "--max-step-face-loops" => ("--max-step-face-loops", &mut limits.max_step_face_loops),
        "--max-step-face-vertices" => (
            "--max-step-face-vertices",
            &mut limits.max_step_face_vertices,
        ),
        "--max-step-assembly-nodes" => (
            "--max-step-assembly-nodes",
            &mut limits.max_step_assembly_nodes,
        ),
        _ => return Ok(false),
    };
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| format!("{option} requires a positive integer"))?;
    let parsed = parse_positive_u64(option, value)?;
    *target = usize::try_from(parsed).map_err(|_| format!("{option} is too large"))?;
    Ok(true)
}

/// Builds import options shared by conversion and batch preflight.
fn build_import_options(settings: &CliConversionSettings) -> ImportOptions {
    ImportOptions {
        resolve_dirs: settings.resolve_dirs.clone(),
        reference_path_mappings: settings.reference_path_mappings.clone(),
        max_lod_error: settings.max_lod_error,
        limits: settings.limits,
        ..ImportOptions::default()
    }
}

/// Builds conversion options from parsed CLI flags without changing conversion semantics.
fn build_conversion_options(
    settings: &CliConversionSettings,
    write_metadata: bool,
    metadata_path: Option<PathBuf>,
) -> ConversionOptions {
    let mut options = ConversionOptions {
        write_metadata,
        metadata_path,
        ..ConversionOptions::default()
    };
    if settings.no_optimize {
        options.mesh.weld_vertices = false;
        options.mesh.rebuild_missing_normals = false;
        options.mesh.position_quantization_step = None;
    } else {
        options.mesh.position_quantization_step = settings.quantize_step;
    }
    options.mesh.max_triangles = settings.max_triangles;
    options.import = build_import_options(settings);
    options.export.include_normals = !settings.omit_normals;
    options
}

/// Builds persisted job settings from the same CLI flags used by conversion.
fn build_job_conversion_settings(
    settings: &CliConversionSettings,
    write_metadata: bool,
) -> JobConversionSettings {
    JobConversionSettings {
        write_metadata,
        optimize_mesh: !settings.no_optimize,
        include_normals: !settings.omit_normals,
        max_triangles: settings.max_triangles,
        quantize_step: settings.quantize_step,
        max_lod_error: settings.max_lod_error,
        resolve_dirs: settings.resolve_dirs.clone(),
        reference_path_mappings: settings
            .reference_path_mappings
            .iter()
            .map(JobReferencePathMapping::from)
            .collect(),
        limits: JobImportLimits::from(settings.limits),
    }
}
