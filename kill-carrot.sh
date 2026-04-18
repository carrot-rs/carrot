#!/bin/bash
pkill -9 -f "carrot" 2>/dev/null
pkill -9 -f "Carrot" 2>/dev/null
pkill -9 -f "cargo.*carrot" 2>/dev/null
echo "All carrot processes killed."
