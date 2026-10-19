# Holder for the magnetic 6-pin pogo connector (see connector.jpg) — generator macro
#
# Run with:  freecadcmd connector_holder.py
# Produces:  ConnectorHolder.FCStd (parametric, spreadsheet-driven),
#            HolderMale.stl, HolderFemale.stl, FitCoupon.stl, ConnectorHolder.step
#
# The connector pair IS the coupler between vehicles: the magnet transmits the
# pulling force and the 6 pogo lanes carry the bus. Convention: male at the
# front, female at the rear of every vehicle.
#
# One holder per wagon end, screwed onto the outer headstock face with 2 × M2
# self-tappers (pilot holes come from wagon_base.py: y = ±holder_screw_y,
# z = conn_axis_z). The holder is a block with a through-bore for the
# connector body and a front recess for the Ø16 face flange, which sits
# `face_proud` in front of the block so mating faces always touch first.
#
# Assembly: feed the pigtails through the bore and the wagon's wire notch,
# epoxy the body into the bore, seat the flange in the recess, screw the
# holder to the headstock. The headstock wall is the back stop (push loads),
# the recess floor takes coupling push, the epoxy takes the magnet pull —
# the wires carry no mechanical load.
#
# flange_t and body length are ASSUMED (photo only) — measure the real part
# and fix them in the Params spreadsheet; the model recomputes.
#
# Local coordinates: connector axis = X axis, back face (headstock side) at
# x = 0, front face at x = depth. Female holder and fit coupon are offset in Y
# just to sit side by side in the document.

import os
import FreeCAD as App
import Part  # noqa: F401  (registers Part types)
import Mesh  # noqa: F401
import MeshPart

OUT = os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals() else os.getcwd()

doc = App.newDocument("ConnectorHolder")

# ---------------------------------------------------------------- parameters
rows = [
    # alias, default, description
    ("flange_d",      16.0,  "connector face flange diameter (both genders)"),
    ("flange_t",      2.0,   "flange thickness -- ASSUMED, measure!"),
    ("male_d",        14.1,  "male body diameter behind the flange"),
    ("female_d",      12.0,  "female body diameter behind the flange"),
    ("flange_clr",    0.6,   "diametral clearance on the flange recess"),
    ("body_clr",      0.3,   "diametral clearance on the body bore"),
    ("face_proud",    1.0,   "how far the mating face sits in front of the block"),
    ("depth",         10,    "block depth along the connector axis"),
    ("block_w",       28,    "block width (>= 2*screw_y + counterbore + wall)"),
    ("block_above",   10,    "block height above the axis (top flush with deck)"),
    ("block_below",   11,    "block height below the axis"),
    ("screw_y",       11,    "screw offset from the axis = wagon holder_screw_y"),
    ("screw_hole",    2.4,   "M2 through hole diameter"),
    ("screw_cb_d",    4.4,   "screw head counterbore diameter"),
    ("screw_cb_t",    5,     "counterbore depth (M2 x 8..10 self-tapper)"),
    ("coupon_t",      4,     "fit test coupon thickness"),
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

def cyl_x(name, r, h, x, y, z):
    o = doc.addObject("Part::Cylinder", name)
    o.setExpression("Radius", r)
    o.setExpression("Height", h)
    o.Placement = App.Placement(App.Vector(), App.Rotation(App.Vector(0, 1, 0), 90))
    o.setExpression(".Placement.Base.x", x)
    o.setExpression(".Placement.Base.y", y)
    o.setExpression(".Placement.Base.z", z)
    return o

def cut(name, base, tools):
    neg = doc.addObject("Part::MultiFuse", name + "Neg")
    neg.Shapes = tools
    o = doc.addObject("Part::Cut", name)
    o.Base = base
    o.Tool = neg
    return o

# ---------------------------------------------------------------- holders
def holder(tag, body_d_alias, y_off):
    """Holder block for one connector gender, offset y_off in the document."""
    blk = box(f"Block{tag}",
              P + "depth", P + "block_w", f"{P}block_above + {P}block_below",
              "0", f"{y_off} - {P}block_w / 2", f"-{P}block_below")
    tools = []
    # body bore, straight through (connector inserted from the front)
    tools.append(cyl_x(f"Bore{tag}",
                       f"({P}{body_d_alias} + {P}body_clr) / 2", f"{P}depth + 2",
                       "-1", y_off, "0"))
    # flange recess in the front face; face sits face_proud ahead of the block
    tools.append(cyl_x(f"FlangeRecess{tag}",
                       f"({P}flange_d + {P}flange_clr) / 2",
                       f"{P}flange_t - {P}face_proud + 1",
                       f"{P}depth - ({P}flange_t - {P}face_proud)", y_off, "0"))
    # M2 mounting screws, counterbored from the front
    for sy, side in ((1, "L"), (-1, "R")):
        tools.append(cyl_x(f"Screw{tag}{side}",
                           f"{P}screw_hole / 2", f"{P}depth + 2",
                           "-1", f"{y_off} + {sy} * {P}screw_y", "0"))
        tools.append(cyl_x(f"ScrewCb{tag}{side}",
                           f"{P}screw_cb_d / 2", f"{P}screw_cb_t + 1",
                           f"{P}depth - {P}screw_cb_t",
                           f"{y_off} + {sy} * {P}screw_y", "0"))
    o = cut(f"Holder{tag}", blk, tools)
    o.Label = f"Holder{tag}"
    return o

male = holder("Male", "male_d", "0")
female = holder("Female", "female_d", "40")

# ---------------------------------------------------------------- fit coupon
# Front-wall slice with one recess + bore per gender — print this first
# (~5 min) to dial in flange_clr / body_clr / face_proud before the holders.
coupon_blk = box("CouponBlock",
                 P + "coupon_t", "52", f"{P}block_above + {P}block_below",
                 "0", "-106", f"-{P}block_below")
coupon_tools = []
for tag, d_alias, cy in (("M", "male_d", "-93"), ("F", "female_d", "-67")):
    coupon_tools.append(cyl_x(f"CouponBore{tag}",
                              f"({P}{d_alias} + {P}body_clr) / 2", f"{P}coupon_t + 2",
                              "-1", cy, "0"))
    coupon_tools.append(cyl_x(f"CouponRecess{tag}",
                              f"({P}flange_d + {P}flange_clr) / 2",
                              f"{P}flange_t - {P}face_proud + 1",
                              f"{P}coupon_t - ({P}flange_t - {P}face_proud)", cy, "0"))
coupon = cut("FitCoupon", coupon_blk, coupon_tools)
coupon.Label = "FitCoupon"

doc.recompute()

# ---------------------------------------------------------------- checks
PARTS = (male, female, coupon)

def check(label):
    errs = [o.Name for o in doc.Objects if "Invalid" in o.State or "Error" in str(o.State)]
    assert not errs, f"{label}: recompute errors in {errs}"
    for p in PARTS:
        s = p.Shape
        assert s.isValid(), f"{label}: {p.Label} shape invalid"
        assert len(s.Solids) == 1, f"{label}: {p.Label} has {len(s.Solids)} solids (expected 1)"
        print(f"[ok] {label}: {p.Label} valid, 1 solid, "
              f"bbox {s.BoundBox.XLength:.1f} x {s.BoundBox.YLength:.1f} x {s.BoundBox.ZLength:.1f} mm")

check("defaults")

# parametric sanity check: thicker flange, recompute, restore
sheet.set("B2", "2.8")   # flange_t
doc.recompute()
check("flange_t 2.8")

sheet.set("B2", "2.0")
doc.recompute()
check("restored")

# ---------------------------------------------------------------- outputs
if App.GuiUp:
    for o in doc.Objects:
        if o not in PARTS and hasattr(o, "ViewObject"):
            o.ViewObject.Visibility = False

doc.saveAs(os.path.join(OUT, "ConnectorHolder.FCStd"))

for obj, fname in ((male, "HolderMale.stl"), (female, "HolderFemale.stl"),
                   (coupon, "FitCoupon.stl")):
    m = MeshPart.meshFromShape(Shape=obj.Shape, LinearDeflection=0.1, AngularDeflection=0.3)
    m.write(os.path.join(OUT, fname))

Part.export(list(PARTS), os.path.join(OUT, "ConnectorHolder.step"))
print("[ok] wrote ConnectorHolder.FCStd / .step / .stl files to", OUT)
