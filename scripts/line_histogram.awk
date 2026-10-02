#!/usr/bin/awk -f
# line_histogram.awk - Profile file by line sizes or extract lines for agents to not floor their context inspecting database extracts such as jsonl files. 
#
# Usage:
#   ./line_histogram.awk <file>                                         # Histogram mode (default)
#   ./line_histogram.awk -v mode=extract -v line=5 <file>               # Extract single line
#   ./line_histogram.awk -v mode=extract -v start=10 -v end=20 <file>   # Extract range
#
# Modes:
#   histogram (default) - Show byte distribution across 10 buckets
#   extract             - Extract specific line(s) with -v line=N or -v start=X -v end=Y

BEGIN {
    # Default to histogram mode if not specified
    if (mode == "") mode = "histogram"
    
    total_bytes = 0
    total_lines = 0
    
    # Determine output destination
    if (outfile == "") {
        out = "/dev/stdout"
    } else {
        out = outfile
    }
}

{
    total_lines++
    line_sizes[total_lines] = length($0)
    lines[total_lines] = $0
    total_bytes += line_sizes[total_lines]
}

END {
    # Handle extract mode
    if (mode == "extract") {
        if (line != "") {
            # Single line extraction
            if (line >= 1 && line <= total_lines) {
                print lines[line]
            } else {
                print "Error: line " line " out of range (1-" total_lines ")" > "/dev/stderr"
                exit 1
            }
        } else if (start != "" && end != "") {
            # Range extraction
            if (start < 1) start = 1
            if (end > total_lines) end = total_lines
            if (start > end) {
                print "Error: start " start " > end " end > "/dev/stderr"
                exit 1
            }
            for (i = start; i <= end; i++) {
                print lines[i]
            }
        } else {
            print "Error: extract mode requires -v line=N or -v start=X -v end=Y" > "/dev/stderr"
            exit 1
        }
        exit 0
    }
    
    # Histogram mode (default)
    print "File: " FILENAME > out
    print "Total bytes: " total_bytes > out
    print "Total lines: " total_lines > out
    print "" > out
    print "Bucket Distribution:" > out
    print "" > out
    
    if (total_lines == 0) {
        print "Empty file" > out
        exit 0
    }
    
    # Determine bucket count and size
    if (total_lines <= 10) {
        num_buckets = total_lines
        bucket_size = 1
    } else {
        num_buckets = 10
        bucket_size = int(total_lines / 10)
        # Handle remainder by making last bucket larger
    }
    
    # Initialize bucket byte counts
    for (i = 1; i <= 10; i++) {
        bucket_bytes[i] = 0
    }
    
    # Assign lines to buckets and sum bytes
    for (line_num = 1; line_num <= total_lines; line_num++) {
        if (total_lines <= 10) {
            bucket = line_num
        } else {
            bucket = int((line_num - 1) / bucket_size) + 1
            if (bucket > 10) bucket = 10  # Last bucket catches remainder
        }
        bucket_bytes[bucket] += line_sizes[line_num]
    }
    
    # Find max bytes for scaling the visual histogram
    max_bytes = 0
    for (i = 1; i <= num_buckets; i++) {
        if (bucket_bytes[i] > max_bytes) max_bytes = bucket_bytes[i]
    }
    
    # Print histogram header and table
    printf "%-15s | %-12s | %-40s\n", "Line Range", "Bytes", "Distribution" > out
    print "─────────────────┼──────────────┼──────────────────────────────────────────" > out
    
    # Print each bucket
    for (i = 1; i <= 10; i++) {
        if (total_lines <= 10) {
            if (i <= total_lines) {
                start_line = i
                end_line = i
            } else {
                start_line = 0
                end_line = 0
            }
        } else {
            start_line = (i - 1) * bucket_size + 1
            if (i == 10) {
                end_line = total_lines
            } else {
                end_line = i * bucket_size
            }
        }
        
        # Format line range
        if (start_line == 0) {
            range = sprintf("%7s", "-")
        } else if (start_line == end_line) {
            range = sprintf("%7d", start_line)
        } else {
            range = sprintf("%d-%d", start_line, end_line)
        }
        
        # Calculate bar length (max 40 chars)
        if (max_bytes > 0) {
            bar_len = int((bucket_bytes[i] / max_bytes) * 40)
        } else {
            bar_len = 0
        }
        
        # Build bar
        bar = ""
        for (j = 1; j <= bar_len; j++) {
            bar = bar "█"
        }
        
        # Print bucket line
        printf "%-15s | %12d | %s\n", range, bucket_bytes[i], bar > out
    }
    print "─────────────────┼──────────────┼──────────────────────────────────────────" > out
}
