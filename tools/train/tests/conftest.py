"""The training scripts are flat modules imported by name (``from ppo
import gae``); put their directory on the path so the tests import the
real modules under test rather than copies."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
