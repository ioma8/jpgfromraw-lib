use anyhow::Result;
use jpgfromraw::parser::{process_file_bytes, FindJpegType};
use std::path::Path;
use std::time::Instant;
use tokio::fs;

/// Path to a directory containing test RAW files
const TEST_RAW_DIR: &str = "/Users/jakubkolcar/Pictures/2024/2024-12-24";

#[tokio::test]
async fn test_process_file_bytes_on_directory() -> Result<()> {
    // Ensure the test directory exists
    if !Path::new(TEST_RAW_DIR).exists() {
        eprintln!("Test directory not found: {}", TEST_RAW_DIR);
        eprintln!("Please create this directory with some RAW files for testing");
        return Ok(());
    }

    let mut entries = fs::read_dir(TEST_RAW_DIR).await?;
    let mut success_count = 0;
    let mut failure_count = 0;
    let mut total_size = 0;
    let overall_start = Instant::now();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        
        // Skip non-files and files without extensions
        if !path.is_file() || path.extension().is_none() {
            continue;
        }
        
        println!("Processing: {}", path.display());
        let file_start = Instant::now();
        
        match process_file_bytes(&path, FindJpegType::Largest).await {
            Ok(jpeg_data) => {
                success_count += 1;
                total_size += jpeg_data.len();
                println!("✅ Success: {} bytes in {:?}", jpeg_data.len(), file_start.elapsed());
            }
            Err(e) => {
                failure_count += 1;
                println!("❌ Failed: {:?} - {}", path.file_name().unwrap(), e);
            }
        }
        
        println!("---");
    }
    
    let total_time = overall_start.elapsed();
    println!("Test complete:");
    println!("  Processed: {} files", success_count + failure_count);
    println!("  Successful: {} files", success_count);
    println!("  Failed: {} files", failure_count);
    println!("  Total JPEG data: {} bytes", total_size);
    println!("  Total time: {:?}", total_time);
    
    if failure_count > 0 {
        println!("⚠️  Warning: Some files failed to process");
    }
    
    Ok(())
}

#[tokio::test]
async fn test_process_file_bytes_with_different_find_types() -> Result<()> {
    // Ensure the test directory exists
    if !Path::new(TEST_RAW_DIR).exists() {
        eprintln!("Test directory not found: {}", TEST_RAW_DIR);
        eprintln!("Please create this directory with some RAW files for testing");
        return Ok(());
    }

    let mut entries = fs::read_dir(TEST_RAW_DIR).await?;
    let mut files_tested = 0;
    
    // Test only first 5 files to keep the test run time reasonable
    'outer: while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        
        if !path.is_file() || path.extension().is_none() {
            continue;
        }
        
        println!("\nTesting both FindJpegType variants on: {}", path.display());
        
        // Test with Largest
        match process_file_bytes(&path, FindJpegType::Largest).await {
            Ok(largest_jpeg) => {
                println!("Largest JPEG size: {} bytes", largest_jpeg.len());
                
                // Test with Smallest
                match process_file_bytes(&path, FindJpegType::Smallest).await {
                    Ok(smallest_jpeg) => {
                        println!("Smallest JPEG size: {} bytes", smallest_jpeg.len());
                        
                        // Verify the types work as expected
                        if largest_jpeg.len() >= smallest_jpeg.len() {
                            println!("✅ Verified: Largest >= Smallest");
                        } else {
                            println!("❌ Error: Largest < Smallest");
                        }
                    }
                    Err(e) => println!("❌ Failed to get smallest JPEG: {}", e),
                }
            }
            Err(e) => println!("❌ Failed to get largest JPEG: {}", e),
        }
        
        files_tested += 1;
        if files_tested >= 5 {
            break 'outer;
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_process_file_bytes_performance_benchmark() -> Result<()> {
    // Ensure the test directory exists
    if !Path::new(TEST_RAW_DIR).exists() {
        eprintln!("Test directory not found: {}", TEST_RAW_DIR);
        eprintln!("Please create this directory with some RAW files for testing");
        return Ok(());
    }

    let mut entries = fs::read_dir(TEST_RAW_DIR).await?;
    let mut times = Vec::new();
    let mut sizes = Vec::new();
    let mut count = 0;
    
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        
        if !path.is_file() || path.extension().is_none() {
            continue;
        }
        
        // Limit to 20 files for the benchmark
        if count >= 20 {
            break;
        }
        
        let start = Instant::now();
        match process_file_bytes(&path, FindJpegType::Largest).await {
            Ok(jpeg_data) => {
                let elapsed = start.elapsed();
                times.push(elapsed);
                sizes.push(jpeg_data.len());
                count += 1;
                println!("Processed file {}: {} bytes in {:?}", 
                    path.file_name().unwrap_or_default().to_string_lossy(), 
                    jpeg_data.len(), 
                    elapsed);
            }
            Err(e) => {
                // Skip failed files in benchmark
                println!("❌ Failed to process {}: {}", 
                    path.file_name().unwrap_or_default().to_string_lossy(), 
                    e);
                continue;
            }
        }
    }
    
    if !times.is_empty() {
        let total_time: u128 = times.iter().map(|t| t.as_millis()).sum();
        let avg_time = total_time as f64 / times.len() as f64;
        let total_size: usize = sizes.iter().sum();
        let avg_size = total_size as f64 / sizes.len() as f64;
        
        println!("Performance benchmark results:");
        println!("  Files processed: {}", times.len());
        println!("  Average processing time: {:.2} ms", avg_time);
        println!("  Average JPEG size: {:.2} KB", avg_size / 1024.0);
        println!("  Throughput: {:.2} MB/s", (total_size as f64 / (total_time as f64 / 1000.0)) / (1024.0 * 1024.0));
    } else {
        println!("No files were successfully processed for benchmark");
    }
    
    Ok(())
}