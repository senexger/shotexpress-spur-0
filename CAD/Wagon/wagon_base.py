# Parametric Gauge 0 (Spur 0) wagon base — generator macro
#
# Run with:  freecadcmd wagon_base.py
# Produces:  WagonBase.FCStd (parametric, spreadsheet-driven),
#            WagonBase.stl, ModulePlateBlank.stl, WagonBase.step
#
# All dimensions live in the "Params" spreadsheet inside the document.
# Every primitive is bound to the spreadsheet via expressions, so editing
# e.g. Params.length or Params.width in the FreeCAD GUI recomputes the
# whole model — no need to re-run this script (it only exists to
# regenerate from scratch or tweak the topology).
#
# Bearing: F683ZZ flanged ball bearing, d=3, D=7, B=3, flange D1=8.1, C1=0.8
#
# Coordinate system: origin at wagon center, Z=0 is the platform top,
# +Z up. Axles run along Y.

import os
import FreeCAD as App
import Part  # noqa: F401  (registers Part types)
import Mesh  # noqa: F401
import MeshPart

OUT = os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals() else os.getcwd()

doc = App.newDocument("WagonBase")

# ---------------------------------------------------------------- parameters
rows = [
    # alias, default, description
    ("length",        180,   "overall frame length"),
    ("width",         60,    "overall frame width (min ~42)"),
    ("wheelbase",     "=length * 0.7", "axle spacing, scales with length; override with a number"),
    ("wall",          2.5,   "skirt wall thickness"),
    ("skirt_h",       12,    "skirt height below platform top"),
    ("deck_t",        4,     "deck rim thickness (module ledge = deck_t - module_t)"),
    ("module_t",      2,     "module plate thickness / seat depth"),
    ("border",        2,     "visible frame border around the module opening"),
    ("ledge_w",       2,     "support ledge width under the module"),
    ("axle_drop",     17,    "axle center below platform top"),
    ("wheel_d",       22,    "wheel diameter (for clearance wells only)"),
    ("ped_span",      27,    "distance over the outer pedestal faces = over bearing flanges"),
    ("ped_t",         4.5,   "pedestal thickness (bearing sits 3.05 deep)"),
    ("ped_w",         12,    "pedestal width along X"),
    ("ped_below",     5.5,   "pedestal material below axle center (>= pocket radius + ~2)"),
    ("bridge_t",      10,    "axle bridge thickness along X"),
    ("brg_od",        7,     "bearing outer diameter (F683ZZ)"),
    ("brg_flange_od", 8.1,   "bearing flange diameter"),
    ("brg_w",         3,     "bearing width"),
    ("brg_flange_t",  0.8,   "bearing flange thickness"),
    ("fit_clr",       0.15,  "diametral press-fit allowance on the bearing pocket"),
    ("axle_clr",      3.8,   "axle clearance bore diameter (axle = 3.0)"),
    ("screw_d",       1.8,   "pilot hole dia for M2 self-tapping module screws"),
    ("mod_screw_d",   2.2,   "through hole dia in the module plate"),
    ("conn_axis_z",   -10,   "magnetic connector axis height (holder top flush with deck)"),
    ("holder_screw_y", 11,   "connector holder screw offset from centerline"),
    ("mod_clr",       0.4,   "total clearance of module plate in its seat"),
]

sheet = doc.addObject("Spreadsheet::Sheet", "Params")
for i, (alias, _val, desc) in enumerate(rows, start=1):
    sheet.set(f"A{i}", alias)
    sheet.setAlias(f"B{i}", alias)
    sheet.set(f"C{i}", desc)
for i, (_alias, val, _desc) in enumerate(rows, start=1):
    sheet.set(f"B{i}", str(val))
doc.recompute()

# ---------------------------------------------------------------- helpers
P = "Params."

def box(name, l, w, h, x, y, z):
    o = doc.addObject("Part::Box", name)
    o.setExpression("Length", l)
    o.setExpression("Width", w)
    o.setExpression("Height", h)
    o.setExpression(".Placement.Base.x", x)
    o.setExpression(".Placement.Base.y", y)
    o.setExpression(".Placement.Base.z", z)
    return o

def cyl(name, r, h, x, y, z, axis="Z"):
    o = doc.addObject("Part::Cylinder", name)
    o.setExpression("Radius", r)
    o.setExpression("Height", h)
    if axis == "Y":   # extends toward +Y from its base point
        o.Placement = App.Placement(App.Vector(), App.Rotation(App.Vector(1, 0, 0), -90))
    elif axis == "X":  # extends toward +X
        o.Placement = App.Placement(App.Vector(), App.Rotation(App.Vector(0, 1, 0), 90))
    o.setExpression(".Placement.Base.x", x)
    o.setExpression(".Placement.Base.y", y)
    o.setExpression(".Placement.Base.z", z)
    return o

def fuse(name, shapes):
    o = doc.addObject("Part::MultiFuse", name)
    o.Shapes = shapes
    return o

def cut(name, base, tool):
    o = doc.addObject("Part::Cut", name)
    o.Base = base
    o.Tool = tool
    return o

SIGNS = (1, -1)

# ---------------------------------------------------------------- shell
shell = box("Shell",
            P+"length", P+"width", P+"skirt_h",
            f"-{P}length / 2", f"-{P}width / 2", f"-{P}skirt_h")

# hollow out below the deck (open bottom = wiring space)
cavity = box("CavityCut",
             f"{P}length - 2 * {P}wall", f"{P}width - 2 * {P}wall",
             f"{P}skirt_h - {P}deck_t + 1",
             f"-{P}length / 2 + {P}wall", f"-{P}width / 2 + {P}wall",
             f"-{P}skirt_h - 1")

# module seat: rebate around the top ...
rebate = box("RebateCut",
             f"{P}length - 2 * {P}border", f"{P}width - 2 * {P}border",
             f"{P}module_t + 0.5",
             f"-{P}length / 2 + {P}border", f"-{P}width / 2 + {P}border",
             f"-{P}module_t")

# ... and the through opening inside the ledge
through = box("ThroughCut",
              f"{P}length - 2 * {P}border - 2 * {P}ledge_w",
              f"{P}width - 2 * {P}border - 2 * {P}ledge_w",
              f"{P}deck_t - {P}module_t + 0.1",
              f"-{P}length / 2 + {P}border + {P}ledge_w",
              f"-{P}width / 2 + {P}border + {P}ledge_w",
              f"-{P}deck_t - 0.1")

base_shell = cut("BaseShell", shell, fuse("ShellNeg", [cavity, rebate, through]))

positives = [base_shell]
negatives = []

# ---------------------------------------------------------------- axles
for sx in SIGNS:
    ax = f"{sx} * {P}wheelbase / 2"
    tag = "F" if sx > 0 else "R"

    # bridge: wall-to-wall cross member, its top is the central module support
    positives.append(box(f"Bridge{tag}",
                         P+"bridge_t", f"{P}width - 2 * {P}wall + 1", "7",
                         f"{ax} - {P}bridge_t / 2",
                         f"-{P}width / 2 + {P}wall - 0.5",
                         "-9"))

    # wire pass-through under the module, through each bridge
    negatives.append(cyl(f"BridgeWireHole{tag}", "2.5", f"{P}bridge_t + 2",
                         f"{ax} - {P}bridge_t / 2 - 1", "0", "-6", axis="X"))

    # module screw pilot holes in the bridge top
    for sy in SIGNS:
        negatives.append(cyl(f"BridgeScrew{tag}{'L' if sy>0 else 'R'}",
                             f"{P}screw_d / 2", "8.5",
                             ax, f"{sy} * 6", "-9.5"))

    # axle clearance bore straight through both pedestals
    negatives.append(cyl(f"AxleBore{tag}", f"{P}axle_clr / 2", f"{P}ped_span + 2",
                         ax, f"-{P}ped_span / 2 - 1", f"-{P}axle_drop", axis="Y"))

    for sy in SIGNS:
        side = "L" if sy > 0 else "R"
        ped_y = f"{P}ped_span / 2 - {P}ped_t" if sy > 0 else f"-{P}ped_span / 2"

        # bearing pedestal hanging from the bridge
        positives.append(box(f"Pedestal{tag}{side}",
                             P+"ped_w", P+"ped_t",
                             f"{P}axle_drop + {P}ped_below - {P}module_t",
                             f"{ax} - {P}ped_w / 2", ped_y,
                             f"-{P}axle_drop - {P}ped_below"))

        # bearing seat, inserted from the outer face:
        # flange recess (flush flange) + bearing pocket
        if sy > 0:
            rec_y = f"{P}ped_span / 2 - {P}brg_flange_t - 0.05"
            poc_y = f"{P}ped_span / 2 - {P}brg_w - 0.15"
        else:
            rec_y = f"-{P}ped_span / 2 - 0.5"
            poc_y = f"-{P}ped_span / 2 - 0.5"
        negatives.append(cyl(f"FlangeRecess{tag}{side}",
                             f"({P}brg_flange_od + 0.3) / 2",
                             f"{P}brg_flange_t + 0.55",
                             ax, rec_y, f"-{P}axle_drop", axis="Y"))
        negatives.append(cyl(f"BearingPocket{tag}{side}",
                             f"({P}brg_od + {P}fit_clr) / 2",
                             f"{P}brg_w + 0.65",
                             ax, poc_y, f"-{P}axle_drop", axis="Y"))

        # wheel clearance well: cuts the bridge arm and (on narrow wagons)
        # notches the skirt wall above the wheel
        well_y = f"{P}ped_span / 2 + 0.3" if sy > 0 else f"-{P}ped_span / 2 - 9.5"
        negatives.append(box(f"WheelWell{tag}{side}",
                             f"{P}wheel_d + 5", "9.2",
                             f"{P}axle_drop + {P}wheel_d - {P}deck_t - 0.4",
                             f"{ax} - {P}wheel_d / 2 - 2.5", well_y,
                             f"-{P}axle_drop - {P}wheel_d"))

# ---------------------------------------------------------------- module corner bosses
for sx in SIGNS:
    for sy in SIGNS:
        bx = f"{P}length / 2 - 8" if sx > 0 else f"-{P}length / 2 + 2"
        by = f"{P}width / 2 - 8" if sy > 0 else f"-{P}width / 2 + 2"
        positives.append(box(f"CornerBoss{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                             "6", "6", "6", bx, by, "-8"))
        negatives.append(cyl(f"CornerScrew{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                             f"{P}screw_d / 2", "7.5",
                             f"{sx} * ({P}length / 2 - 5.5)",
                             f"{sy} * ({P}width / 2 - 5.5)", "-8.5"))

# ---------------------------------------------------------------- connector holder interface (both ends)
# Magnetic 6-pin connector sits in a separate printed holder (connector_holder.py)
# screwed onto the outer headstock face: 2 pilot holes + an open-bottom wire
# notch through the end wall. The end wall is the holder's back stop.
for sx in SIGNS:
    tag = "F" if sx > 0 else "R"
    hx = f"{P}length / 2 - {P}wall - 1" if sx > 0 else f"-{P}length / 2 - 1"
    for sy in SIGNS:
        negatives.append(cyl(f"HolderScrew{tag}{'L' if sy>0 else 'R'}",
                             f"{P}screw_d / 2", f"{P}wall + 2",
                             hx, f"{sy} * {P}holder_screw_y",
                             P+"conn_axis_z", axis="X"))
    negatives.append(box(f"WireNotch{tag}",
                         f"{P}wall + 2", "8", "5.5",
                         hx, "-4", f"-{P}skirt_h - 0.5"))

# ---------------------------------------------------------------- wire management loops
loop_centers = [f"{P}wheelbase / 4", f"{P}length / 2 - 13"]
for ci, c in enumerate(loop_centers):
    for sx in SIGNS:
        for sy in SIGNS:
            name = f"Loop{ci}{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}"
            ly = f"{P}width / 2 - {P}wall - 2.5" if sy > 0 else f"-{P}width / 2 + {P}wall - 0.5"
            positives.append(box(name, "9", "3", "5",
                                 f"{sx} * ({c}) - 4.5", ly, "-11"))
            sy_ = f"{P}width / 2 - {P}wall - 3" if sy > 0 else f"-{P}width / 2 + {P}wall - 0.3"
            negatives.append(box(name + "Slot", "5", "3.3", "2.4",
                                 f"{sx} * ({c}) - 2.5", sy_, "-9.7"))

# ---------------------------------------------------------------- booleans
body = fuse("BodyFuse", positives)
neg = fuse("Negatives", negatives)
final = cut("WagonBase", body, neg)
final.Label = "WagonBase"

# ---------------------------------------------------------------- blank module plate (interface template)
plate = box("PlateBlank",
            f"{P}length - 2 * {P}border - {P}mod_clr",
            f"{P}width - 2 * {P}border - {P}mod_clr",
            P+"module_t",
            f"-{P}length / 2 + {P}border + {P}mod_clr / 2",
            f"-{P}width / 2 + {P}border + {P}mod_clr / 2",
            f"-{P}module_t")
plate_holes = []
for sx in SIGNS:
    for sy in SIGNS:
        plate_holes.append(cyl(f"PlateHoleB{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                               f"{P}mod_screw_d / 2", f"{P}module_t + 1",
                               f"{sx} * {P}wheelbase / 2", f"{sy} * 6",
                               f"-{P}module_t - 0.5"))
        plate_holes.append(cyl(f"PlateHoleC{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                               f"{P}mod_screw_d / 2", f"{P}module_t + 1",
                               f"{sx} * ({P}length / 2 - 5.5)",
                               f"{sy} * ({P}width / 2 - 5.5)",
                               f"-{P}module_t - 0.5"))
plate_final = cut("ModulePlateBlank", plate, fuse("PlateNeg", plate_holes))
plate_final.Label = "ModulePlateBlank"

doc.recompute()

# ---------------------------------------------------------------- checks
def check(label):
    errs = [o.Name for o in doc.Objects if "Invalid" in o.State or "Error" in str(o.State)]
    assert not errs, f"{label}: recompute errors in {errs}"
    s = final.Shape
    assert s.isValid(), f"{label}: WagonBase shape invalid"
    assert len(s.Solids) == 1, f"{label}: WagonBase has {len(s.Solids)} solids (expected 1)"
    assert plate_final.Shape.isValid(), f"{label}: plate shape invalid"
    print(f"[ok] {label}: valid, 1 solid, "
          f"bbox {s.BoundBox.XLength:.1f} x {s.BoundBox.YLength:.1f} x {s.BoundBox.ZLength:.1f} mm, "
          f"volume {s.Volume/1000:.1f} cm3")

check("defaults 180x60")

# parametric sanity check: resize, recompute, restore
sheet.set("B1", "220")   # length
sheet.set("B2", "45")    # width
doc.recompute()
check("resized 220x45")

sheet.set("B1", "180")
sheet.set("B2", "60")
doc.recompute()
check("restored 180x60")

# ---------------------------------------------------------------- outputs
if App.GuiUp:
    for o in doc.Objects:
        if o not in (final, plate_final) and hasattr(o, "ViewObject"):
            o.ViewObject.Visibility = False

doc.saveAs(os.path.join(OUT, "WagonBase.FCStd"))

for obj, fname in ((final, "WagonBase.stl"), (plate_final, "ModulePlateBlank.stl")):
    m = MeshPart.meshFromShape(Shape=obj.Shape, LinearDeflection=0.1, AngularDeflection=0.3)
    m.write(os.path.join(OUT, fname))

Part.export([final, plate_final], os.path.join(OUT, "WagonBase.step"))
print("[ok] wrote WagonBase.FCStd / .step / .stl files to", OUT)
