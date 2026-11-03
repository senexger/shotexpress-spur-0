# Parametric Gauge 0 (Spur 0) wagon base — generator macro
#
# Run with:  freecadcmd wagon_base.py
# Produces:  WagonBase.FCStd (parametric, spreadsheet-driven),
#            WagonBase.stl, ModulePlateBlank.stl, ModuleCoal.stl,
#            ModuleCoalDecorated.stl, ModuleCoalLid.stl, ModuleShot.stl,
#            ModuleCoal2.stl, ModuleCoal2Lid.stl,
#            WagonBase.step
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
#
# Modules mount flat ON TOP of the frame (no recessed seat): the deck rim,
# both axle bridges and the four corner bosses all end flush at Z=0, and a
# module plate the size of the whole frame is screwed down onto them.

import os
import FreeCAD as App
import Part  # noqa: F401  (registers Part types)
import Mesh  # noqa: F401
import MeshPart

OUT = os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals() else os.getcwd()

doc = App.newDocument("WagonBase")

# ---------------------------------------------------------------- parameters
# Kategorien mit (Alias, Wert, Erklärung). Aliase ohne Umlaute (FreeCAD-Regel).
categories = [
    ("Rahmen", [
        ("laenge",            150,   "Gesamtlänge des Rahmens (ohne Kupplung)"),
        ("breite",            55,    "Gesamtbreite des Rahmens (mind. ca. 42)"),
        ("radstand",          "=laenge * 0.7 - 6", "Abstand der beiden Achsen; wächst mit der Länge, oder feste Zahl eintragen"),
        ("wandstaerke",       3,     "Dicke der Seiten- und Stirnwände (Schürze)"),
        ("schuerzenhoehe",    12,    "Höhe der Schürze unter der Plattform-Oberkante"),
        ("randtiefe",         4,     "Tiefe des oberen Rands (Innenlippe unter der Modulplatte)"),
        ("randbreite",        4,     "Breite des flachen Rands, auf dem die Modulplatte aufliegt"),
    ]),
    ("Module", [
        ("modulplatte_dicke", 2,     "Dicke der Modulplatte (liegt oben auf, z = 0 .. Dicke)"),
        ("vorbohrung_d",      1.8,   "Vorbohrung im Rahmen für M2-Blechschrauben"),
        ("modul_bohrung_d",   2.2,   "Durchgangsbohrung in der Modulplatte für M2"),
        ("mitte_x",           0,     "Position der zusätzlichen Befestigung in der Modulmitte (2 Schrauben an den Seitenwänden)"),
    ]),
    ("Räder & Achsen", [
        ("rad_d",             27,    "Raddurchmesser (bestimmt die Radkästen)"),
        ("achstiefe",         17.5,  "Achsmitte unter Plattform-Oberkante; Plattformhöhe über Schiene = achstiefe + rad_d/2"),
        ("radspiel",          1.5,   "Luft zwischen Radoberkante und dem Brückensteg darüber"),
        ("achsbohrung_d",     3.8,   "Freibohrung für die Achse (Achse = 3,0)"),
        ("bruecke_dicke",     10,    "Dicke der Achsbrücke in Fahrtrichtung"),
    ]),
    ("Lagerbock", [
        ("lagerbock_abstand", 27,    "Maß über die Außenseiten der Lagerböcke = über die Lagerflansche; an Radsatz anpassen"),
        ("lagerbock_dicke",   4.5,   "Dicke des Lagerbocks quer zur Fahrtrichtung (Lager sitzt 3,05 tief)"),
        ("lagerbock_breite",  18,    "Breite des Lagerbocks in Fahrtrichtung"),
        ("lagerbock_unten",   5.5,   "Material unter der Achsmitte (>= Lagerradius + ca. 2)"),
    ]),
    ("Kugellager F683ZZ", [
        ("lager_d",           7,     "Außendurchmesser des Lagers"),
        ("lager_flansch_d",   8.1,   "Durchmesser des Lagerflansches"),
        ("lager_breite",      3,     "Breite des Lagers"),
        ("lager_flansch_dicke", 0.8, "Dicke des Lagerflansches"),
        ("presspassung",      0.15,  "Zugabe auf den Lagersitz-Durchmesser für die Presspassung"),
    ]),
    ("Kupplung", [
        ("kupplung_z",        -10,   "Höhe der Magnetkupplungs-Achse (Halter oben bündig mit dem Deck)"),
        ("halter_schraube_y", 11,    "Abstand der Halter-Schrauben von der Mittellinie"),
        ("schnur_d",          4,     "Durchmesser des Schnurlochs in den Stirnwänden (vorläufige Kupplung)"),
        ("schnur_z",          -7,    "Höhe der Schnurloch-Mitte"),
    ]),
    ("Shotmodul", [
        ("shot_hoehe",        15,    "Gesamthöhe des Shotmoduls (liegt statt des Kohlemoduls oben auf)"),
        ("shot_boden",        3,     "Bodendicke unter den Gläsern"),
        ("glas_d",            35,    "Lochdurchmesser (ohne Spiel) auf Höhe glas_d_hoehe; das Loch ist konisch wie das Glas"),
        ("glas_d_hoehe",      "=shot_hoehe / 2", "Höhe ab Modulunterseite, auf der das Loch glas_d misst"),
        ("glas_konus",        6,     "Neigung der Lochwand in Grad (Shotgläser sind unten schmaler)"),
        ("glas_spiel",        0.6,   "Luft im Lochdurchmesser"),
        ("glas_abstand",      "=laenge / 3", "Mittenabstand der drei Glaslöcher"),
        ("glas_fase",         1,     "Fase oben am Loch, damit das Glas leicht hineinrutscht"),
        ("shot_schraube_tiefe", 8,   "Tiefe der Vorbohrungen von unten (Schrauben kommen von unten durch den Rahmen)"),
        ("abfluss_d",         3,     "Durchmesser der Abflusslöcher unter den äußeren Gläsern (senkrecht nach unten)"),
        ("abfluss_mitte_rand", 2,    "mittleres Glas: Abflussloch so groß, dass ringsum nur dieser Bodenrand stehen bleibt"),
        ("abfluss_versatz",   14,    "Abflussloch der äußeren Gläser liegt so weit zur Wagenmitte hin (nicht über Achsbrücke und Rädern)"),
    ]),
    ("Kohlemodul", [
        ("wanne_hoehe",       60,    "Höhe der Wannenwände über der Bodenplatte (Innenraum für die Elektronik)"),
        ("wanne_wand",        1.8,   "Wandstärke der Wanne"),
        ("wanne_einzug",      1.5,   "Abstand Wannen-Außenwand zur Plattenkante (Platz für Deckelschürze und Deko)"),
        ("kabel_l",           14,    "Kabeldurchführung im Boden (Mitte), Länge in Fahrtrichtung"),
        ("kabel_b",           6,     "Kabeldurchführung im Boden, Breite"),
    ]),
    ("Kohlemodul Deckel", [
        ("deckel_dicke",      1.2,   "Dicke der Deckelplatte unter der Kohle"),
        ("kappe_hoehe",       4,     "Höhe des Deckelrands (Stahlträger) ab Deckelunterseite; die Kohle liegt darin"),
        ("tropfnut_tiefe",    0.5,   "Tiefe der Tropfnut unter dem Deckelüberstand (Wasser tropft dort ab statt nach innen zu kriechen)"),
        ("stift_d",           1.6,   "Durchmesser der zwei Zentrierstifte auf der Wanne"),
        ("stift_h",           1,     "Höhe der Zentrierstifte"),
        ("stift_spiel",       0.15,  "Spiel der Stiftlöcher im Deckel, je Seite"),
    ]),
    ("Kohleladung", [
        ("kohle_rand",        1,   "Kohle liegt am Rand so viel unter der Oberkante des Deckelrands"),
        ("kohle_haufen",      7,     "Kohlehaufen ist in der Mitte so viel höher als am Rand"),
        ("kohle_min",         1.2,   "kleinste Kohlestück-Größe"),
        ("kohle_max",         4.5,   "größte Kohlestück-Größe"),
        ("kohle_raster",      0.3,   "Auflösung der Kohleoberfläche (kleiner = feiner, größere Datei)"),
        ("kohle_seed",        7,     "Zufallswert: andere Zahl = andere Kohle. Kohle wird nur beim Ausführen von wagon_base.py neu erzeugt"),
    ]),
    ("Lüftungsschlitze", [
        ("schlitz_laenge",    8,    "Länge der waagrechten Lüftungsschlitze"),
        ("schlitz_hoehe",     1.2,   "Höhe der Schlitze (dünn halten gegen Wasser)"),
        ("schlitz_anstieg",   1.5,   "Schlitz ist innen so viel höher als außen; eindringendes Wasser läuft nach außen ab"),
        ("schlitz_abstand",   14,    "Mittenabstand der Schlitzspalten entlang der Wand (Anzahl wird beim Erzeugen festgelegt)"),
        ("schlitz_reihen",    3,     "Schlitzreihen je Gruppe: unten Zuluft, oben Abluft (Anzahl wird beim Erzeugen festgelegt)"),
        ("schlitz_reihenabstand", 3.5, "senkrechter Abstand der Schlitzreihen"),
        ("schlitz_z",         5,     "Unterkante der untersten Reihe außen, ab Oberseite Bodenplatte"),
        ("schlitz_oben_abstand", 1.5, "Abstand der obersten Reihe unter dem oberen Band"),
    ]),
    ("Kohlemodul Deko", [
        ("deko_tiefe",        0.8,   "so weit stehen die Stahlträger außen vor"),
        ("deko_traeger",      2.5,   "Breite aller Stahlträger: Bänder, Gurte und Eckpfosten"),
        ("deckel_nut_tiefe",  0.4,   "Tiefe der Nut im Deckelrand (Deckelrand sieht aus wie ein U-Träger)"),
        ("deckel_flansch",    0.8,   "Höhe der oberen und unteren Flansche am Deckelrand"),
        ("deko_oben_abstand", 0.3,   "Luft zwischen oberem Band und Deckel"),
    ]),
    ("Kohlemodul 2 (nach Skizze)", [
        ("k2_knick",          0.35,  "Knick zwischen schrägem unterem und senkrechtem oberem Wandteil, Anteil der Wannenhöhe"),
        ("k2_haut",           0.6,   "so weit steht der obere Wandteil vor; darunter läuft er schräg auf 0 aus (Trichterform)"),
        ("k2_pfosten",        3,     "Breite der senkrechten Pfosten"),
        ("k2_pfosten_tiefe",  0.7,   "so weit stehen Pfosten, Bänder, Streben und Rippen über den oberen Wandteil vor"),
        ("k2_feld_innen",     0.17,  "Lage der inneren Pfosten als Anteil der Wannenlänge (dazwischen das Schild)"),
        ("k2_feld_aussen",    0.34,  "Lage der äußeren Pfosten als Anteil der Wannenlänge"),
        ("k2_strebe",         4,     "waagrechte Breite der Diagonalstreben"),
        ("k2_schild_h",       15,    "Höhe des Namensschilds"),
        ("k2_schild_z",       "=modulplatte_dicke + wanne_hoehe * 0.6", "Höhe der Schildmitte"),
        ("k2_schild_text1",   "COAL TRANSPORT", "Schild, obere Zeile (Schrift wird nur beim Ausführen von wagon_base.py neu erzeugt)"),
        ("k2_schild_text2",   "POWER SYSTEMS", "Schild, untere Zeile, mit Ein-Symbol davor"),
        ("k2_rippen",         7,     "Anzahl Kühlrippen an den Stirnseiten (wird beim Erzeugen festgelegt)"),
        ("k2_rippen_abstand", 4.5,   "Mittenabstand der Kühlrippen"),
        ("k2_rippe_dicke",    1,     "Dicke der Kühlrippen"),
        ("led_d",             3.1,   "Löcher für 3-mm-LEDs oben an den Ecken der Stirnseiten"),
    ]),
]

sheet = doc.addObject("Spreadsheet::Sheet", "Params")
sheet.set("A1", "Parameter"); sheet.set("B1", "Wert"); sheet.set("C1", "Erklärung")
sheet.setStyle("A1:C1", "bold")
sheet.setColumnWidth("A", 160); sheet.setColumnWidth("C", 520)
row, cells = 2, []
for cat, items in categories:
    row += 1                          # empty line before each category
    sheet.set(f"A{row}", cat)
    sheet.setStyle(f"A{row}", "bold|underline")
    row += 1
    for alias, val, desc in items:
        sheet.set(f"A{row}", alias)
        sheet.setAlias(f"B{row}", alias)
        sheet.set(f"C{row}", desc)
        cells.append((row, val))
        row += 1
for r, val in cells:                  # values last, so "=laenge*..." resolves
    sheet.set(f"B{r}", str(val))
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

# Refine (merging coplanar faces) is off: it only tidies the face layout,
# the printed STL is the same, and it costs seconds on the decorated parts.
def fuse(name, shapes):
    o = doc.addObject("Part::MultiFuse", name)
    o.Shapes = shapes
    o.Refine = False
    return o

def cut(name, base, tool):
    o = doc.addObject("Part::Cut", name)
    o.Base = base
    o.Tool = tool
    o.Refine = False
    return o

SIGNS = (1, -1)

# ---------------------------------------------------------------- shell
shell = box("Shell",
            P+"laenge", P+"breite", P+"schuerzenhoehe",
            f"-{P}laenge / 2", f"-{P}breite / 2", f"-{P}schuerzenhoehe")

# hollow out below the deck (open bottom = wiring space)
cavity = box("CavityCut",
             f"{P}laenge - 2 * {P}wandstaerke", f"{P}breite - 2 * {P}wandstaerke",
             f"{P}schuerzenhoehe - {P}randtiefe + 1",
             f"-{P}laenge / 2 + {P}wandstaerke", f"-{P}breite / 2 + {P}wandstaerke",
             f"-{P}schuerzenhoehe - 1")

# open top inside the flat rim (wires pass up into the module anywhere)
through = box("ThroughCut",
              f"{P}laenge - 2 * {P}randbreite", f"{P}breite - 2 * {P}randbreite",
              f"{P}randtiefe + 0.2",
              f"-{P}laenge / 2 + {P}randbreite", f"-{P}breite / 2 + {P}randbreite",
              f"-{P}randtiefe - 0.1")

base_shell = cut("BaseShell", shell, fuse("ShellNeg", [cavity, through]))

positives = [base_shell]
negatives = []

# ---------------------------------------------------------------- axles
for sx in SIGNS:
    ax = f"{sx} * {P}radstand / 2"
    tag = "F" if sx > 0 else "R"

    # bridge: wall-to-wall cross member, top flush with the deck = module support
    positives.append(box(f"Bridge{tag}",
                         P+"bruecke_dicke", f"{P}breite - 2 * {P}wandstaerke + 1", "9",
                         f"{ax} - {P}bruecke_dicke / 2",
                         f"-{P}breite / 2 + {P}wandstaerke - 0.5",
                         "-9"))

    # wire pass-through under the module, through each bridge
    negatives.append(cyl(f"BridgeWireHole{tag}", "2.5", f"{P}bruecke_dicke + 2",
                         f"{ax} - {P}bruecke_dicke / 2 - 1", "0", "-6", axis="X"))

    # module screw pilot holes in the bridge top
    for sy in SIGNS:
        negatives.append(cyl(f"BridgeScrew{tag}{'L' if sy>0 else 'R'}",
                             f"{P}vorbohrung_d / 2", "9",
                             ax, f"{sy} * 6", "-8.5"))

    # axle clearance bore straight through both pedestals
    negatives.append(cyl(f"AxleBore{tag}", f"{P}achsbohrung_d / 2", f"{P}lagerbock_abstand + 2",
                         ax, f"-{P}lagerbock_abstand / 2 - 1", f"-{P}achstiefe", axis="Y"))

    for sy in SIGNS:
        side = "L" if sy > 0 else "R"
        ped_y = f"{P}lagerbock_abstand / 2 - {P}lagerbock_dicke" if sy > 0 else f"-{P}lagerbock_abstand / 2"

        # bearing pedestal hanging from the bridge
        positives.append(box(f"Pedestal{tag}{side}",
                             P+"lagerbock_breite", P+"lagerbock_dicke",
                             f"{P}achstiefe + {P}lagerbock_unten",
                             f"{ax} - {P}lagerbock_breite / 2", ped_y,
                             f"-{P}achstiefe - {P}lagerbock_unten"))

        # bearing seat, inserted from the outer face:
        # flange recess (flush flange) + bearing pocket
        if sy > 0:
            rec_y = f"{P}lagerbock_abstand / 2 - {P}lager_flansch_dicke - 0.05"
            poc_y = f"{P}lagerbock_abstand / 2 - {P}lager_breite - 0.15"
        else:
            rec_y = f"-{P}lagerbock_abstand / 2 - 0.5"
            poc_y = f"-{P}lagerbock_abstand / 2 - 0.5"
        negatives.append(cyl(f"FlangeRecess{tag}{side}",
                             f"({P}lager_flansch_d + 0.3) / 2",
                             f"{P}lager_flansch_dicke + 0.55",
                             ax, rec_y, f"-{P}achstiefe", axis="Y"))
        negatives.append(cyl(f"BearingPocket{tag}{side}",
                             f"({P}lager_d + {P}presspassung) / 2",
                             f"{P}lager_breite + 0.65",
                             ax, poc_y, f"-{P}achstiefe", axis="Y"))

        # wheel clearance well: cuts the bridge arm up to wheel top + wheel_clr
        # (the arm left above it ties pedestal and skirt wall together) and,
        # on narrow wagons, notches the skirt wall beside the wheel
        well_y = f"{P}lagerbock_abstand / 2 + 0.1" if sy > 0 else f"-{P}lagerbock_abstand / 2 - 9.3"
        negatives.append(box(f"WheelWell{tag}{side}",
                             f"{P}rad_d + 5", "9.2",
                             f"{P}rad_d * 1.5 + {P}radspiel",
                             f"{ax} - {P}rad_d / 2 - 2.5", well_y,
                             f"-{P}achstiefe - {P}rad_d"))

# ---------------------------------------------------------------- module corner bosses
for sx in SIGNS:
    for sy in SIGNS:
        bx = f"{P}laenge / 2 - 8" if sx > 0 else f"-{P}laenge / 2 + 2"
        by = f"{P}breite / 2 - 8" if sy > 0 else f"-{P}breite / 2 + 2"
        positives.append(box(f"CornerBoss{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                             "6", "6", "8", bx, by, "-8"))
        negatives.append(cyl(f"CornerScrew{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                             f"{P}vorbohrung_d / 2", "9",
                             f"{sx} * ({P}laenge / 2 - 5.5)",
                             f"{sy} * ({P}breite / 2 - 5.5)", "-8.5"))

# ---------------------------------------------------------------- module middle bosses
# extra module screws in the middle of each side, at (mitte_x, ±(breite/2 - 5.5))
for sy in SIGNS:
    t = "L" if sy > 0 else "R"
    positives.append(box(f"MidBoss{t}", "6", "6", "8",
                         f"{P}mitte_x - 3",
                         f"{P}breite / 2 - 8" if sy > 0 else f"-{P}breite / 2 + 2", "-8"))
    negatives.append(cyl(f"MidScrew{t}", f"{P}vorbohrung_d / 2", "9",
                         P+"mitte_x", f"{sy} * ({P}breite / 2 - 5.5)", "-8.5"))

# ---------------------------------------------------------------- connector holder interface (both ends)
# Magnetic 6-pin connector sits in a separate printed holder (connector_holder.py)
# screwed onto the outer headstock face: 2 pilot holes + an open-bottom wire
# notch through the end wall. The end wall is the holder's back stop.
for sx in SIGNS:
    tag = "F" if sx > 0 else "R"
    hx = f"{P}laenge / 2 - {P}wandstaerke - 1" if sx > 0 else f"-{P}laenge / 2 - 1"
    for sy in SIGNS:
        negatives.append(cyl(f"HolderScrew{tag}{'L' if sy>0 else 'R'}",
                             f"{P}vorbohrung_d / 2", f"{P}wandstaerke + 2",
                             hx, f"{sy} * {P}halter_schraube_y",
                             P+"kupplung_z", axis="X"))
    # TEMPORARY until the connector holder is used: a closed cord hole in the
    # end wall (thread a cord through, knot it inside from the open underside)
    # instead of the open-bottom connector wire notch.
    negatives.append(cyl(f"CordHole{tag}", f"{P}schnur_d / 2", f"{P}wandstaerke + 2",
                         hx, "0", P+"schnur_z", axis="X"))
    # negatives.append(box(f"WireNotch{tag}",
    #                      f"{P}wandstaerke + 2", "8", "5.5",
    #                      hx, "-4", f"-{P}schuerzenhoehe - 0.5"))

# ---------------------------------------------------------------- booleans
body = fuse("BodyFuse", positives)
neg = fuse("Negatives", negatives)
final = cut("WagonBase", body, neg)
final.Label = "WagonBase"

# ---------------------------------------------------------------- blank module plate (interface template)
# full frame footprint, lies flat on top of the frame (z = 0 .. module_t)
plate = box("PlateBlank",
            P+"laenge", P+"breite", P+"modulplatte_dicke",
            f"-{P}laenge / 2", f"-{P}breite / 2", "0")
plate_holes = []
for sx in SIGNS:
    for sy in SIGNS:
        plate_holes.append(cyl(f"PlateHoleB{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                               f"{P}modul_bohrung_d / 2", f"{P}modulplatte_dicke + 1",
                               f"{sx} * {P}radstand / 2", f"{sy} * 6",
                               "-0.5"))
        plate_holes.append(cyl(f"PlateHoleC{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                               f"{P}modul_bohrung_d / 2", f"{P}modulplatte_dicke + 1",
                               f"{sx} * ({P}laenge / 2 - 5.5)",
                               f"{sy} * ({P}breite / 2 - 5.5)",
                               "-0.5"))
for sy in SIGNS:
    plate_holes.append(cyl(f"PlateHoleM{'L' if sy>0 else 'R'}",
                           f"{P}modul_bohrung_d / 2", f"{P}modulplatte_dicke + 1",
                           P+"mitte_x", f"{sy} * ({P}breite / 2 - 5.5)", "-0.5"))
plate_final = cut("ModulePlateBlank", plate, fuse("PlateNeg", plate_holes))
plate_final.Label = "ModulePlateBlank"

# ---------------------------------------------------------------- shot glass module
# Alternative to the coal module: a 15 mm block over the whole frame with
# three blind pockets for shot glasses. Prints floor down, no supports.
# Conical pockets (narrow at the bottom, like the glasses). Screwed from
# BELOW through the frame: blind pilot holes from the underside at the 4
# corners and the 2 middle positions, nothing visible from above. The 4
# bridge positions are not used (only the thin pocket floor is above them).
def cone(name, r1, r2, h, x, y, z):
    o = doc.addObject("Part::Cone", name)
    o.setExpression("Radius1", r1)
    o.setExpression("Radius2", r2)
    o.setExpression("Height", h)
    o.setExpression(".Placement.Base.x", x)
    o.setExpression(".Placement.Base.y", y)
    o.setExpression(".Placement.Base.z", z)
    return o

shot_blk = box("ShotBlock", P+"laenge", P+"breite", P+"shot_hoehe",
               f"-{P}laenge / 2", f"-{P}breite / 2", "0")
GR = f"({P}glas_d + {P}glas_spiel) / 2"
def glas_r(z):   # pocket radius at height z
    return f"({GR} + (({z}) - {P}glas_d_hoehe) * tan({P}glas_konus))"
R_FLOOR = glas_r(P+"shot_boden")
shot_neg = []
for i, n in enumerate(("R", "M", "F")):
    gx = f"{i - 1} * {P}glas_abstand"
    shot_neg.append(cone(f"ShotGlas{n}", R_FLOOR, glas_r(f"{P}shot_hoehe + 1"),
                         f"{P}shot_hoehe - {P}shot_boden + 1", gx, "0", P+"shot_boden"))
    rf = glas_r(f"{P}shot_hoehe - {P}glas_fase")
    shot_neg.append(cone(f"ShotFase{n}", rf, f"{rf} + 2 * {P}glas_fase", f"2 * {P}glas_fase",
                         gx, "0", f"{P}shot_hoehe - {P}glas_fase"))
for sx, sy in [(sx, sy) for sx in SIGNS for sy in SIGNS] + [(0, 1), (0, -1)]:
    t = f"{'F' if sx>0 else 'R' if sx<0 else 'M'}{'L' if sy>0 else 'R'}"
    px = P+"mitte_x" if sx == 0 else f"{sx} * ({P}laenge / 2 - 5.5)"
    shot_neg.append(cyl(f"ShotPilot{t}", f"{P}vorbohrung_d / 2", f"{P}shot_schraube_tiefe + 0.5",
                        px, f"{sy} * ({P}breite / 2 - 5.5)", "-0.5"))
# drains: straight down through the floor. Middle glass in its centre; the
# outer glasses off-centre toward the wagon middle, so the liquid falls
# through the open frame between the bridges, not onto bridge, axle or wheels.
shot_neg.append(cyl("ShotAbflussM", f"{R_FLOOR} - {P}abfluss_mitte_rand", f"{P}shot_boden + 1", "0", "0", "-0.5"))
for n, gx in (("F", f"{P}glas_abstand - {P}abfluss_versatz"),
              ("R", f"-({P}glas_abstand - {P}abfluss_versatz)")):
    shot_neg.append(cyl(f"ShotAbfluss{n}", f"{P}abfluss_d / 2", f"{P}shot_boden + 1", gx, "0", "-0.5"))
shot = cut("ModuleShot", shot_blk, fuse("ShotNeg", shot_neg))
shot.Label = "ModuleShot"

# ---------------------------------------------------------------- coal wagon module
# A closed tub screwed onto the frame from above (same 8 screw positions as
# the blank plate, which is its floor), holding the train electronics, plus a
# lid that looks like the coal load. Rain protection: the lid's outer skirt
# overlaps the tub walls, the coal sits in a closed basin, and the vent slots
# climb from outside to inside so water that gets in runs back out.
#
#   ModuleCoal           plain tub                          print floor down
#   ModuleCoalDecorated  same tub + Bänder, Gurte, Eckpfosten
#   ModuleCoalLid        lid frame (rim, skirt, lip, ribs)  print as mounted
#   CoalLoad             coal surface mesh (generated), exported together
#                        with the lid frame as ModuleCoalLid.stl
TL = f"({P}laenge - 2 * {P}wanne_einzug)"          # tub outer length
TB = f"({P}breite - 2 * {P}wanne_einzug)"          # tub outer width
FT = f"{P}modulplatte_dicke"                        # floor top
TOP = f"({P}modulplatte_dicke + {P}wanne_hoehe)"    # tub rim
BAND_TOP = f"({TOP} - {P}deko_oben_abstand)"       # decor ends just under the lid overhang

coal_floor = box("CoalFloor", P+"laenge", P+"breite", FT,
                 f"-{P}laenge / 2", f"-{P}breite / 2", "0")
coal_walls = box("CoalWalls", TL, TB, P+"wanne_hoehe",
                 f"-{TL} / 2", f"-{TB} / 2", FT)

coal_neg = [box("CoalInside",
                f"{TL} - 2 * {P}wanne_wand", f"{TB} - 2 * {P}wanne_wand", f"{P}wanne_hoehe + 1",
                f"-{TL} / 2 + {P}wanne_wand", f"-{TB} / 2 + {P}wanne_wand", FT)]
for sx in SIGNS:
    for sy in SIGNS:
        t = f"{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}"
        coal_neg.append(cyl(f"CoalHoleB{t}", f"{P}modul_bohrung_d / 2", f"{FT} + 1",
                            f"{sx} * {P}radstand / 2", f"{sy} * 6", "-0.5"))
        coal_neg.append(cyl(f"CoalHoleC{t}", f"{P}modul_bohrung_d / 2", f"{FT} + 1",
                            f"{sx} * ({P}laenge / 2 - 5.5)", f"{sy} * ({P}breite / 2 - 5.5)", "-0.5"))
for sy in SIGNS:
    coal_neg.append(cyl(f"CoalHoleM{'L' if sy>0 else 'R'}", f"{P}modul_bohrung_d / 2", f"{FT} + 1",
                        P+"mitte_x", f"{sy} * ({P}breite / 2 - 5.5)", "-0.5"))
coal_neg.append(box("CoalCableHole", P+"kabel_l", P+"kabel_b", f"{FT} + 1",
                    f"-{P}kabel_l / 2", f"-{P}kabel_b / 2", "-0.5"))
coal_neg_base = list(coal_neg)          # without vent slots (for ModuleCoal2)

# slot columns/rows are fixed here from the default size; positions follow the sheet
_L, _B, _E = float(sheet.laenge), float(sheet.breite), float(sheet.wanne_einzug)
_pitch, _sl = float(sheet.schlitz_abstand), float(sheet.schlitz_laenge)
N_LONG = int((_L - 2 * _E - 12 - _sl) / _pitch) + 1
N_END = int((_B - 2 * _E - 12 - _sl) / _pitch) + 1
N_ROWS = int(float(sheet.schlitz_reihen))
def _pos(i, n):
    return f"({i} - {(n - 1) / 2}) * {P}schlitz_abstand"
def _gap(i, n):   # centre between column i and i+1
    return f"({i} - {(n - 2) / 2}) * {P}schlitz_abstand"
ROW_Z = ([f"{FT} + {P}schlitz_z + {r} * {P}schlitz_reihenabstand" for r in range(N_ROWS)] +
         [f"{BAND_TOP} - {P}deko_traeger - {P}schlitz_oben_abstand - {P}schlitz_hoehe"
          f" - {r} * {P}schlitz_reihenabstand" for r in range(N_ROWS)])

ROT_LONG = App.Rotation(App.Vector(1, 0, 0), 90)                     # face in XZ
ROT_END = App.Rotation(App.Vector(0, 0, 1), 90).multiply(ROT_LONG)   # face in YZ
slots = []
def vent_slot(name, along_x, sign, pos, z):
    """Thin horizontal slot through a tub wall; its inner opening sits
    schlitz_anstieg higher than the outer one, so water runs back out."""
    pl = doc.addObject("Part::Plane", name + "Face")
    pl.setExpression("Length", P+"schlitz_laenge")
    pl.setExpression("Width", P+"schlitz_hoehe")
    pl.Placement = App.Placement(App.Vector(), ROT_LONG if along_x else ROT_END)
    if along_x:   # long side wall at y = sign * TB/2
        pl.setExpression(".Placement.Base.x", f"{pos} - {P}schlitz_laenge / 2")
        pl.setExpression(".Placement.Base.y", f"{sign} * ({TB} / 2 + 0.2)")
    else:         # end wall at x = sign * TL/2
        pl.setExpression(".Placement.Base.x", f"{sign} * ({TL} / 2 + 0.2)")
        pl.setExpression(".Placement.Base.y", f"{pos} - {P}schlitz_laenge / 2")
    pl.setExpression(".Placement.Base.z", z)
    ex = doc.addObject("Part::Extrusion", name)
    ex.Base = pl
    ex.DirMode = "Custom"
    ex.Solid = True
    ex.LengthFwd = 0   # use the length of Dir
    ex.Dir = App.Vector(0, -sign, 1) if along_x else App.Vector(-sign, 0, 1)
    ex.setExpression(".Dir.y" if along_x else ".Dir.x", f"{-sign} * ({P}wanne_wand + 0.4)")
    ex.setExpression(".Dir.z", f"{P}schlitz_anstieg * ({P}wanne_wand + 0.4) / {P}wanne_wand")
    slots.append(ex)
    return ex

for ri, z in enumerate(ROW_Z):
    for sy in SIGNS:
        for i in range(N_LONG):
            coal_neg.append(vent_slot(f"VentL{'L' if sy>0 else 'R'}{ri}_{i}", True, sy, _pos(i, N_LONG), z))
    for sx in SIGNS:
        for i in range(N_END):
            coal_neg.append(vent_slot(f"VentE{'F' if sx>0 else 'R'}{ri}_{i}", False, sx, _pos(i, N_END), z))

# lid locating: a sloped bracket under the rim of each end wall (printable
# without support), each carrying a pin that drops into a hole in the flat
# lid underside. Placed in a gap between slot columns, diagonally opposite,
# so the lid fits either way round.
PIN_Y = {1: _gap(N_END - 2, N_END) if N_END > 1 else "0",
         -1: _gap(0, N_END) if N_END > 1 else "0"}
pins = []
for sx in SIGNS:
    t = "F" if sx > 0 else "R"
    w = doc.addObject("Part::Wedge", f"PinBracket{t}")
    # local Y = height, local X = inward (negative), local Z = across the wall
    w.Xmin, w.Xmax, w.Ymin, w.Ymax, w.Zmin, w.Zmax = -0.2, 0.1, 0, 6, -1.7, 1.7
    w.X2min, w.X2max, w.Z2min, w.Z2max = -4, 0.1, -1.7, 1.7
    rot = App.Rotation(App.Vector(1, 0, 0), 90)
    if sx < 0:
        rot = App.Rotation(App.Vector(0, 0, 1), 180).multiply(rot)
    w.Placement = App.Placement(App.Vector(), rot)
    w.setExpression(".Placement.Base.x", f"{sx} * ({TL} / 2 - {P}wanne_wand)")
    w.setExpression(".Placement.Base.y", PIN_Y[sx])
    w.setExpression(".Placement.Base.z", f"{TOP} - 6")
    pins.append(w)
    pins.append(cyl(f"LidPin{t}", f"{P}stift_d / 2", f"{P}stift_h + 0.1",
                    f"{sx} * ({TL} / 2 - {P}wanne_wand - 2)", PIN_Y[sx], f"{TOP} - 0.1"))

coal_negs = fuse("CoalNeg", coal_neg)
coal_plain = cut("ModuleCoal", fuse("CoalBody", [coal_floor, coal_walls] + pins), coal_negs)
coal_plain.Label = "ModuleCoal"

# ---- decoration: riveted-steel look. All beams have the same width
# (deko_traeger): bottom and top band, two belts between the slot groups,
# and the four corner posts.
T = P+"deko_tiefe"
BW = P+"deko_traeger"
RUNGE_H = f"{BAND_TOP} - {FT}"
GURT_LO = f"({FT} + {P}schlitz_z + {N_ROWS - 1} * {P}schlitz_reihenabstand + {P}schlitz_hoehe + 1.5)"
GURT_HI = (f"({BAND_TOP} - {BW} - {P}schlitz_oben_abstand - {P}schlitz_hoehe"
           f" - {N_ROWS - 1} * {P}schlitz_reihenabstand - 1.5 - {BW})")
BAND_Z = [FT, GURT_LO, GURT_HI, f"{BAND_TOP} - {BW}"]          # bottom of each horizontal beam

deco = []
for s_ in SIGNS:
    k = "L" if s_ > 0 else "R"
    kk = "F" if s_ > 0 else "R"
    y0 = f"{TB} / 2 - 0.2" if s_ > 0 else f"-{TB} / 2 - {T}"
    x0 = f"{TL} / 2 - 0.2" if s_ > 0 else f"-{TL} / 2 - {T}"
    for j, z in enumerate(BAND_Z):
        deco.append(box(f"DecoBand{j}{k}", f"{TL} + 2 * {T}", f"{T} + 0.2", BW,
                        f"-{TL} / 2 - {T}", y0, z))
        deco.append(box(f"DecoBand{j}{kk}", f"{T} + 0.2", TB, BW, x0, f"-{TB} / 2", z))
for sx in SIGNS:
    for sy in SIGNS:
        deco.append(box(f"DecoEcke{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}",
                        f"{BW} + {T}", f"{BW} + {T}", RUNGE_H,
                        f"{TL} / 2 - {BW}" if sx > 0 else f"-{TL} / 2 - {T}",
                        f"{TB} / 2 - {BW}" if sy > 0 else f"-{TB} / 2 - {T}",
                        FT))

coal_deco = cut("ModuleCoalDecorated",
                fuse("CoalDecoBody", [coal_floor, coal_walls] + pins + deco),
                coal_negs)
coal_deco.Label = "ModuleCoalDecorated"

# ---- lid: flat underside (prints coal-up straight on the bed, no supports).
# A slab the size of the plate, overhanging the tub walls by wanne_einzug,
# with a rim (kappe_hoehe) around the coal. The rim is dressed as a
# U-channel. Under the overhang, outside the walls, a drip groove stops
# water from creeping inward; inside the walls the underside is plain flat.
RI_L = f"({TL} - 2 * {P}wanne_wand)"      # rim inner = tub inner wall line = coal area
RI_B = f"({TB} - 2 * {P}wanne_wand)"
RIM_TOP = f"({TOP} + {P}kappe_hoehe)"
lid_body = cut("LidBody",
               box("LidSlab", P+"laenge", P+"breite", P+"kappe_hoehe",
                   f"-{P}laenge / 2", f"-{P}breite / 2", TOP),
               box("LidCoalBay", RI_L, RI_B, P+"kappe_hoehe",
                   f"-{RI_L} / 2", f"-{RI_B} / 2", f"{TOP} + {P}deckel_dicke"))
lid_neg = []
# drip groove: 0.6 wide, 0.5 .. 1.1 mm in from the lid edge (wall face is at wanne_einzug)
lid_neg.append(cut("LidDrip",
                   box("LidDripOuter", f"{P}laenge - 1", f"{P}breite - 1", f"{P}tropfnut_tiefe + 0.1",
                       f"-({P}laenge - 1) / 2", f"-({P}breite - 1) / 2", f"{TOP} - 0.1"),
                   box("LidDripInner", f"{P}laenge - 2.2", f"{P}breite - 2.2", f"{P}tropfnut_tiefe + 1",
                       f"-({P}laenge - 2.2) / 2", f"-({P}breite - 2.2) / 2", f"{TOP} - 0.5")))
# pin holes (through the slab; the coal above closes them)
for sx in SIGNS:
    lid_neg.append(cyl(f"LidPinHole{'F' if sx > 0 else 'R'}", f"{P}stift_d / 2 + {P}stift_spiel",
                       f"{P}deckel_dicke + 0.1",
                       f"{sx} * ({TL} / 2 - {P}wanne_wand - 2)", PIN_Y[sx], f"{TOP} - 0.1"))
# U-channel groove around the rim, flanges top and bottom
NUT_Z0 = f"{TOP} + {P}deckel_flansch"
NUT_H = f"{P}kappe_hoehe - 2 * {P}deckel_flansch"
NT = P+"deckel_nut_tiefe"
for s_ in SIGNS:
    k = "L" if s_ > 0 else "R"
    kk = "F" if s_ > 0 else "R"
    lid_neg.append(box(f"LidNut{k}", f"{P}laenge - 2 * {BW}", f"{NT} + 0.1", NUT_H,
                       f"-{P}laenge / 2 + {BW}",
                       f"{P}breite / 2 - {NT}" if s_ > 0 else f"-{P}breite / 2 - 0.1", NUT_Z0))
    lid_neg.append(box(f"LidNut{kk}", f"{NT} + 0.1", f"{P}breite - 2 * {BW}", NUT_H,
                       f"{P}laenge / 2 - {NT}" if s_ > 0 else f"-{P}laenge / 2 - 0.1",
                       f"-{P}breite / 2 + {BW}", NUT_Z0))
lid = cut("ModuleCoalLid", lid_body, fuse("LidNeg", lid_neg))
lid.Label = "ModuleCoalLid"

# ---------------------------------------------------------------- coal module, second approach
# Same tub, floor, screws, pins and lid fit as ModuleCoal, but no vent slots
# and styled after coal_wagon_sketch.jpeg: hopper walls (lower part leans in,
# upper part vertical), posts, X-braces, a nameplate with raised lettering,
# cooling fins on the ends, LED holes at the top corners, and a lid whose
# rim is held by clamp straps. No rivets (too slow to build).
#
#   ModuleCoal2       tub with all decoration          print floor down
#   ModuleCoal2Lid    lid with clamp straps            exported with CoalLoad
#
# Everything on the walls is a Part::Wedge laid on one wall face: along the
# wall (a), up (z) and outward (thickness, which may change from bottom to top).
ZW = App.Vector(0, 0, 1)
FACES = {   # outward normal, wall position, along-axis, sign of local X vs. world along-axis
    "L": (App.Vector(0, 1, 0), f"{TB} / 2", "x", -1),
    "R": (App.Vector(0, -1, 0), f"{TB} / 2", "x", 1),
    "F": (App.Vector(1, 0, 0), f"{TL} / 2", "y", 1),
    "B": (App.Vector(-1, 0, 0), f"{TL} / 2", "y", -1),
}
def face_rot(f):
    n = FACES[f][0]
    u = ZW.cross(n)
    return App.Placement(App.Matrix(u.x, 0, n.x, 0, u.y, 0, n.y, 0, u.z, 1, n.z, 0, 0, 0, 0, 1)).Rotation

def face_base(o, f, z):
    n, wall, ax, _ = FACES[f]
    sgn = int(n.x + n.y)
    o.setExpression(".Placement.Base.x", f"{sgn} * {wall}" if ax == "y" else "0")
    o.setExpression(".Placement.Base.y", f"{sgn} * {wall}" if ax == "x" else "0")
    o.setExpression(".Placement.Base.z", z)

def fw(name, f, a0, a1, z0, z1, t0, t1=None, a0t=None, a1t=None, off="0"):
    """Slab on wall face f: a0..a1 along the wall at z0 (a0t..a1t at z1),
    thickness t0 at z0 growing to t1 at z1, starting off mm out from the wall."""
    t1 = t0 if t1 is None else t1
    a0t = a0 if a0t is None else a0t
    a1t = a1 if a1t is None else a1t
    s = FACES[f][3]
    lo, hi, lot, hit = ((a0, a1, a0t, a1t) if s > 0 else
                        (f"-({a1})", f"-({a0})", f"-({a1t})", f"-({a0t})"))
    w = doc.addObject("Part::Wedge", name)
    w.Placement = App.Placement(App.Vector(), face_rot(f))
    for k, e in (("Xmin", lo), ("Xmax", hi), ("X2min", lot), ("X2max", hit),
                 ("Ymin", "0"), ("Ymax", f"({z1}) - ({z0})"),
                 ("Zmin", f"({off}) - 0.2"), ("Zmax", f"({off}) + ({t0})"),
                 ("Z2min", f"({off}) - 0.2"), ("Z2max", f"({off}) + ({t1})")):
        w.setExpression(k, e)
    face_base(w, f, z0)
    return w

SK, PT, PW = P+"k2_haut", P+"k2_pfosten_tiefe", P+"k2_pfosten"
ST = f"({SK} + {PT})"                                  # full depth: posts, bands, fins
ZK = f"({FT} + {P}wanne_hoehe * {P}k2_knick)"         # knee between hopper slope and vertical part
ZT = BAND_TOP
P1 = f"({TL} * {P}k2_feld_innen)"
P2 = f"({TL} * {P}k2_feld_aussen)"
SLOPE = f"{SK} / ({ZK} - {FT})"                        # skin growth per mm height in the lower part

d2 = []
for f in "LRFB":
    half = f"{TL} / 2" if f in "LR" else f"{TB} / 2"
    # hopper skin: leans in below the knee, vertical above
    d2.append(fw(f"K2Haut{f}0", f, f"-{half}", half, FT, ZK, "0", SK))
    d2.append(fw(f"K2Haut{f}1", f, f"-{half}", half, ZK, ZT, SK))
    # bands: bottom (follows the slope), knee, top. Long sides run over the corners.
    ext0, ext1 = (f"{PT}", ST) if f in "LR" else ("0", "0")
    d2.append(fw(f"K2BandU{f}", f, f"-{half} - {ext0}", f"{half} + {ext0}", FT, f"{FT} + {BW}",
                 PT, f"{PT} + {SLOPE} * {BW}",
                 a0t=f"-{half} - {PT} - {SLOPE} * {BW}" if f in "LR" else None,
                 a1t=f"{half} + {PT} + {SLOPE} * {BW}" if f in "LR" else None))
    for k, z in (("K", ZK), ("O", f"{ZT} - {BW}")):
        d2.append(fw(f"K2Band{k}{f}", f, f"-{half} - {ext1}", f"{half} + {ext1}", z, f"{z} + {BW}", ST))
    # corner posts (long sides wrap round the corner)
    for sa in SIGNS:
        c = "P" if sa > 0 else "N"
        if f in "LR":
            out0, out1 = f"{half} + {PT}", f"{half} + {ST}"
            a = (f"{half} - {PW}", out0, f"{half} - {PW}", out1) if sa > 0 else \
                (f"-({out0})", f"-{half} + {PW}", f"-({out1})", f"-{half} + {PW}")
        else:
            a = (f"{half} - {PW}", half, None, None) if sa > 0 else (f"-{half}", f"-{half} + {PW}", None, None)
        d2.append(fw(f"K2Ecke{f}{c}0", f, a[0], a[1], FT, ZK, PT, ST, a[2], a[3]))
        d2.append(fw(f"K2Ecke{f}{c}1", f, a[0], a[1] if a[3] is None else a[3], ZK, ZT, ST,
                     a0t=None, a1t=None) if sa > 0 or a[2] is None else
                  fw(f"K2Ecke{f}{c}1", f, a[2], a[1], ZK, ZT, ST))

# side posts, braces, nameplate
UP0, UP1 = f"{ZK} + {BW}", f"{ZT} - {BW}"              # upper panel between knee band and top band
SH = P+"k2_schild_h"
SZ0, SZ1 = f"{P}k2_schild_z - {SH} / 2", f"{P}k2_schild_z + {SH} / 2"
SW = P+"k2_strebe"
for f in "LR":
    for sa in SIGNS:
        c = "P" if sa > 0 else "N"
        for j, pc in ((1, P1), (2, P2)):
            a0, a1 = f"{sa} * {pc} - {PW} / 2", f"{sa} * {pc} + {PW} / 2"
            d2.append(fw(f"K2Pfosten{j}{f}{c}0", f, a0, a1, FT, ZK, PT, ST))
            d2.append(fw(f"K2Pfosten{j}{f}{c}1", f, a0, a1, ZK, ZT, ST))
        # end panel: X brace
        e0, e1 = (f"{P2} + {PW} / 2", f"{TL} / 2 - {PW}")
        if sa < 0:
            e0, e1 = f"-{TL} / 2 + {PW}", f"-{P2} - {PW} / 2"
        d2.append(fw(f"K2KreuzA{f}{c}", f, e0, f"{e0} + {SW}", UP0, UP1, PT, off=SK,
                     a0t=f"{e1} - {SW}", a1t=e1))
        d2.append(fw(f"K2KreuzB{f}{c}", f, f"{e1} - {SW}", e1, UP0, UP1, PT, off=SK,
                     a0t=e0, a1t=f"{e0} + {SW}"))
        # middle panel: one brace, low at the nameplate, high at the outer post
        m0, m1 = (f"{P1} + {PW} / 2", f"{P2} - {PW} / 2")
        if sa > 0:
            d2.append(fw(f"K2Strebe{f}{c}", f, m0, f"{m0} + {SW}", UP0, UP1, PT, off=SK,
                         a0t=f"{m1} - {SW}", a1t=m1))
        else:
            d2.append(fw(f"K2Strebe{f}{c}", f, f"-({m0}) - {SW}", f"-({m0})", UP0, UP1, PT, off=SK,
                         a0t=f"-({m1})", a1t=f"-({m1}) + {SW}"))
    # nameplate: thin panel + raised border, short centre straps above and below
    nw = f"({P1} - {PW} / 2 - 2)"
    d2.append(fw(f"K2Schild{f}", f, f"-{nw}", nw, SZ0, SZ1, "0.3", off=SK))
    for k, (a0, a1, z0, z1) in {"u": (f"-{nw}", nw, SZ0, f"{SZ0} + 1.2"),
                                "o": (f"-{nw}", nw, f"{SZ1} - 1.2", SZ1),
                                "l": (f"-{nw}", f"-{nw} + 1.2", SZ0, SZ1),
                                "r": (f"{nw} - 1.2", nw, SZ0, SZ1)}.items():
        d2.append(fw(f"K2Schild{k}{f}", f, a0, a1, z0, z1, PT, off=SK))
    d2.append(fw(f"K2MitteU{f}", f, f"-{PW} / 2", f"{PW} / 2", FT, ZK, PT, ST))
    d2.append(fw(f"K2MitteM{f}", f, f"-{PW} / 2", f"{PW} / 2", ZK, SZ0, ST))
    d2.append(fw(f"K2MitteO{f}", f, f"-{PW} / 2", f"{PW} / 2", SZ1, ZT, ST))

# cooling fins on both ends (count fixed here from the default)
N_RIP = int(float(sheet.k2_rippen))
for f in "FB":
    for i in range(N_RIP):
        yc = f"({i} - {(N_RIP - 1) / 2}) * {P}k2_rippen_abstand"
        d2.append(fw(f"K2Rippe{f}{i}", f, f"{yc} - {P}k2_rippe_dicke / 2", f"{yc} + {P}k2_rippe_dicke / 2",
                     f"{FT} + {BW}", f"{ZT} - {BW}", ST))

# raised lettering on the nameplate (text and font fixed when the script runs)
FONT = "/usr/share/fonts/TTF/DejaVuSansCondensed-Bold.ttf"   # clean outlines (Roboto's overlap)
def text_shape():
    def char_face(ws):
        # even-odd: a contour inside an odd number of bigger contours is a hole
        # outlines as fine polygons (0.01 mm): straight edges fuse much faster
        def poly(w):
            pts = Part.Wire(w).discretize(Deflection=0.01)
            return Part.Face(Part.makePolygon(pts + [pts[0]] if pts[0].distanceToPoint(pts[-1]) > 1e-9 else pts))
        fs = sorted((poly(w) for w in ws), key=lambda f: -f.Area)
        out = None
        for f in fs:
            d = sum(1 for g in fs if g.Area > f.Area and g.isInside(f.Vertexes[0].Point, 1e-6, True))
            out = f if out is None else (out.cut(f) if d % 2 else out.fuse(f))
        return out
    def faces_of(txt, size):
        return Part.makeCompound([char_face(ch) for ch in Part.makeWireString(txt, FONT, size, 0) if ch])
    def power_icon(r):
        disc = lambda rr: Part.Face(Part.Wire(Part.makeCircle(rr)))
        ring = disc(r).cut(disc(r * 0.62))
        ring = ring.cut(Part.Face(Part.makePolygon([App.Vector(-r * 0.45, 0, 0), App.Vector(r * 0.45, 0, 0),
                                                    App.Vector(r * 0.45, r * 1.2, 0), App.Vector(-r * 0.45, r * 1.2, 0),
                                                    App.Vector(-r * 0.45, 0, 0)])))
        bar = Part.Face(Part.makePolygon([App.Vector(-r * 0.19, -r * 0.05, 0), App.Vector(r * 0.19, -r * 0.05, 0),
                                          App.Vector(r * 0.19, r * 1.1, 0), App.Vector(-r * 0.19, r * 1.1, 0),
                                          App.Vector(-r * 0.19, -r * 0.05, 0)]))
        return ring.fuse(bar)
    t1 = str(sheet.get("k2_schild_text1")); t2 = str(sheet.get("k2_schild_text2"))
    l1, l2 = faces_of(t1, 10), faces_of(t2, 10)
    b2 = l2.BoundBox
    ic = power_icon(b2.YLength * 0.5)
    ic.translate(App.Vector(b2.XMin - b2.YLength * 0.9, b2.YMin + b2.YLength * 0.5, 0))
    l2 = Part.makeCompound([l2, ic])
    b1, b2 = l1.BoundBox, l2.BoundBox
    gap = b1.YLength * 0.45
    l1.translate(App.Vector(-b1.Center.x, gap / 2 - b1.YMin, 0))
    l2.translate(App.Vector(-b2.Center.x, -gap / 2 - b2.YMax, 0))
    both = Part.makeCompound([l1, l2])
    # fit into the plate inside the border (1.2) with 1 mm margin
    W = 2 * (float(sheet.k2_feld_innen) * (float(sheet.laenge) - 2 * float(sheet.wanne_einzug))
             - float(sheet.k2_pfosten) / 2 - 2) - 2 * 2.2
    H = float(sheet.k2_schild_h) - 2 * 2.2
    bb = both.BoundBox
    k = min(W / bb.XLength, H / bb.YLength)
    both = both.scale(k)
    bb = both.BoundBox
    out = float(sheet.k2_haut) + 0.3
    sols = []
    for fc in both.Faces:
        so = fc.extrude(App.Vector(0, 0, float(sheet.k2_pfosten_tiefe) - 0.3 + 0.1))
        # move each letter, not the compound: a Part::Feature's own Placement
        # replaces the compound's, but keeps the placements of its parts
        so.translate(App.Vector(-bb.Center.x, -bb.Center.y, out - 0.1))
        sols.append(so)
    return Part.makeCompound(sols)
TXT = text_shape()
for f in "LR":
    o = doc.addObject("Part::Feature", f"K2Schrift{f}")
    # local: x along the wall (reading direction from outside), y up, z out
    o.Shape = TXT
    o.Placement = App.Placement(App.Vector(), face_rot(f))
    face_base(o, f, P+"k2_schild_z")
    # lettering is fused onto its small nameplate first: far faster than
    # fusing every letter into the whole body
    panel = doc.getObject(f"K2Schild{f}")
    i = d2.index(panel)
    d2[i] = fuse(f"K2SchildText{f}", [panel, o])

# LED holes at the top corners of both ends
c2_neg = list(coal_neg_base)
for sx in SIGNS:
    for sy in SIGNS:
        c2_neg.append(cyl(f"K2Led{'F' if sx>0 else 'R'}{'L' if sy>0 else 'R'}", f"{P}led_d / 2",
                          f"{P}wanne_wand + {ST} + 1",
                          f"{TL} / 2 - {P}wanne_wand - 0.5" if sx > 0 else f"-{TL} / 2 - {ST} - 0.5",
                          f"{sy} * ({TB} / 2 - {PW} - 4)", f"{ZT} - {BW} - 3", axis="X"))
# hollow the plain tub first, then add the decoration (it stays outside the
# walls). Only the end skins need the LED holes as well.
coal2_tub = cut("Coal2Tub", fuse("Coal2Base", [coal_floor, coal_walls] + pins), fuse("Coal2Neg", c2_neg))
led_holes = fuse("K2LedHoles", [o for o in c2_neg if o.Name.startswith("K2Led")])
for f in "FB":
    skin = doc.getObject(f"K2Haut{f}1")
    d2[d2.index(skin)] = cut(f"K2Haut{f}1Led", skin, led_holes)
coal2 = fuse("ModuleCoal2", [coal2_tub] + d2)
coal2.Label = "ModuleCoal2"

# ---- lid 2: same lid, the U-channel interrupted by clamp straps that also
# hook over the rim top (at the side posts, and two per end)
NUT_A = {"L": [f"-{P}laenge / 2 + {BW}", f"-{P2} - {PW} / 2", f"-{P2} + {PW} / 2", f"-{P1} - {PW} / 2",
               f"-{P1} + {PW} / 2", f"{P1} - {PW} / 2", f"{P1} + {PW} / 2", f"{P2} - {PW} / 2",
               f"{P2} + {PW} / 2", f"{P}laenge / 2 - {BW}"],
         "F": [f"-{P}breite / 2 + {BW}", f"-{P}breite / 4 - {PW} / 2", f"-{P}breite / 4 + {PW} / 2",
               f"{P}breite / 4 - {PW} / 2", f"{P}breite / 4 + {PW} / 2", f"{P}breite / 2 - {BW}"]}
lid2_neg = [o for o in lid_neg if not o.Name.startswith("LidNut")]
clamps = []
for s_ in SIGNS:
    for side, cuts in NUT_A.items():
        k = (side if s_ > 0 else {"L": "R", "F": "B"}[side])
        for i in range(0, len(cuts), 2):
            a0, a1 = cuts[i], cuts[i + 1]
            if side == "L":
                lid2_neg.append(box(f"Lid2Nut{k}{i}", f"({a1}) - ({a0})", f"{NT} + 0.1", NUT_H, a0,
                                    f"{P}breite / 2 - {NT}" if s_ > 0 else f"-{P}breite / 2 - 0.1", NUT_Z0))
            else:
                lid2_neg.append(box(f"Lid2Nut{k}{i}", f"{NT} + 0.1", f"({a1}) - ({a0})", NUT_H,
                                    f"{P}laenge / 2 - {NT}" if s_ > 0 else f"-{P}laenge / 2 - 0.1", a0, NUT_Z0))
        for i in range(1, len(cuts) - 1, 2):     # strap = gap between two groove pieces
            a0, a1 = cuts[i], cuts[i + 1]
            dep = f"{P}wanne_einzug + {P}wanne_wand + 0.4"
            if side == "L":
                clamps.append(box(f"Lid2Klammer{k}{i}", f"({a1}) - ({a0})", dep, "0.7", a0,
                                  f"{P}breite / 2 - ({dep})" if s_ > 0 else f"-{P}breite / 2",
                                  f"{RIM_TOP} - 0.1"))
            else:
                clamps.append(box(f"Lid2Klammer{k}{i}", dep, f"({a1}) - ({a0})", "0.7",
                                  f"{P}laenge / 2 - ({dep})" if s_ > 0 else f"-{P}laenge / 2", a0,
                                  f"{RIM_TOP} - 0.1"))
lid2 = cut("ModuleCoal2Lid", fuse("Lid2Body", [lid_body] + clamps), fuse("Lid2Neg", lid2_neg))
lid2.Label = "ModuleCoal2Lid"

doc.recompute()

# ---- coal load: procedural lumps -> height field -> closed mesh
# Every lump is a random convex polyhedron (intersection of random half-spaces,
# slightly flattened). Lumps are scattered over a heap-shaped target surface,
# each sunk a random amount, and only the highest surface is kept. That gives
# angular, faceted chunks with deep gaps between them, and no overhangs.
import numpy as np

def make_coal(seed):
    rng = np.random.default_rng(int(seed))
    g = float(sheet.kohle_raster)
    top = 0.0   # built relative to the tub rim; the feature's Placement lifts it to TOP
    base = top + float(sheet.deckel_dicke) - 0.05            # overlaps the plate
    rim_top = top + float(sheet.kappe_hoehe)
    edge = rim_top - float(sheet.kohle_rand)
    heap = float(sheet.kohle_haufen)
    smin, smax = float(sheet.kohle_min), float(sheet.kohle_max)
    hx = (float(sheet.laenge) - 2 * float(sheet.wanne_einzug)) / 2 - float(sheet.wanne_wand) + 0.1
    hy = (float(sheet.breite) - 2 * float(sheet.wanne_einzug)) / 2 - float(sheet.wanne_wand) + 0.1
    xs = np.linspace(-hx, hx, int(round(2 * hx / g)) + 1)
    ys = np.linspace(-hy, hy, int(round(2 * hy / g)) + 1)
    X, Y = np.meshgrid(xs, ys, indexing="ij")
    def target(x, y):   # loaded-wagon heap: flat ends, ridge along the middle
        fx = np.clip(1 - (np.abs(x) / hx) ** 6, 0, 1)
        fy = np.clip(1 - (np.abs(y) / hy) ** 2, 0, 1)
        return edge + heap * fx * fy
    T = target(X, Y)
    H = T - 1.5 * smax                                        # deep gaps between lumps
    area = 4 * hx * hy
    mean = (smin + smax) / 2
    n = int(3.0 * area / mean ** 2)
    for _ in range(n):
        size = smin + (smax - smin) * rng.random() ** 1.6     # more small than big lumps
        r = size / 2
        cx, cy = rng.uniform(-hx, hx), rng.uniform(-hy, hy)
        k = 14
        nrm = rng.normal(size=(k, 3)); nrm /= np.linalg.norm(nrm, axis=1)[:, None]
        off = r * rng.uniform(0.75, 1.0, k)
        nrm[:, 2] /= 0.7                                      # flatten the lump
        cz = target(cx, cy) - rng.uniform(0.1, 0.7) * r
        ix = slice(max(np.searchsorted(xs, cx - 1.6 * r), 0), np.searchsorted(xs, cx + 1.6 * r))
        iy = slice(max(np.searchsorted(ys, cy - 1.6 * r), 0), np.searchsorted(ys, cy + 1.6 * r))
        dx, dy = X[ix, iy] - cx, Y[ix, iy] - cy
        up, dn = nrm[:, 2] > 1e-6, nrm[:, 2] < -1e-6
        if not up.any() or not dn.any():
            continue
        lim = (off[:, None, None] - nrm[:, 0, None, None] * dx - nrm[:, 1, None, None] * dy) / nrm[:, 2, None, None]
        zt = np.minimum(lim[up].min(axis=0), r * rng.uniform(0.45, 0.7))   # flat-ish top cap, no spikes
        zb = np.maximum(lim[dn].max(axis=0), -r)
        inside = zt > zb
        if inside.any():
            sub = H[ix, iy]
            H[ix, iy] = np.where(inside, np.maximum(sub, cz + zt), sub)
    H = np.minimum(H, T + 0.6 * smax)                         # safety cap above the heap
    H = np.maximum(H, base + 0.6)
    # closed mesh: top grid, four side walls, bottom fan
    nx, ny = H.shape
    vid = np.arange(nx * ny).reshape(nx, ny)
    top_v = np.stack([X.ravel(), Y.ravel(), H.ravel()], axis=1)
    a, b, c, d = vid[:-1, :-1].ravel(), vid[1:, :-1].ravel(), vid[1:, 1:].ravel(), vid[:-1, 1:].ravel()
    tris = [np.stack([a, b, c], 1), np.stack([a, c, d], 1)]
    ring = np.concatenate([vid[:, 0], vid[-1, 1:], vid[-2::-1, -1], vid[0, -2:0:-1]])   # CCW loop
    nb = len(ring)
    bot_v = top_v[ring].copy(); bot_v[:, 2] = base
    bot_id = nx * ny + np.arange(nb)
    centre_id = nx * ny + nb
    V = np.vstack([top_v, bot_v, [[0, 0, base]]])
    nxt = np.roll(np.arange(nb), -1)
    tris.append(np.stack([ring, bot_id, bot_id[nxt]], 1))
    tris.append(np.stack([ring, bot_id[nxt], ring[nxt]], 1))
    tris.append(np.stack([np.full(nb, centre_id), bot_id[nxt], bot_id], 1))
    F = np.vstack(tris)
    m = Mesh.Mesh(V[F].tolist())
    if m.Volume < 0:
        m.flipNormals()
    return m

coal_mesh = make_coal(sheet.kohle_seed)
coal_obj = doc.addObject("Mesh::Feature", "CoalLoad")
coal_obj.Mesh = coal_mesh
coal_obj.Label = "CoalLoad"
# position follows the sheet (tub height, plate thickness); the lump surface
# itself (heap, size, seed, footprint) only changes when this script runs
coal_obj.setExpression(".Placement.Base.z", TOP)
doc.recompute()

# ---------------------------------------------------------------- checks
# Quick checks run every time. The slow ones (every vent slot, and a full
# resize/restore cycle) only with:  WAGON_FULL_CHECK=1 freecadcmd wagon_base.py
FULL = os.environ.get("WAGON_FULL_CHECK") == "1"

def check(label):
    errs = [o.Name for o in doc.Objects if "Invalid" in o.State or "Error" in str(o.State)]
    assert not errs, f"{label}: recompute errors in {errs}"
    s = final.Shape
    assert s.isValid(), f"{label}: WagonBase shape invalid"
    assert len(s.Solids) == 1, f"{label}: WagonBase has {len(s.Solids)} solids (expected 1)"
    assert plate_final.Shape.isValid(), f"{label}: plate shape invalid"
    # wheels (Ø wheel_d, 4 mm wide, just outside the bearing flanges) must not touch
    wb, ad, r = float(sheet.radstand), float(sheet.achstiefe), float(sheet.rad_d) / 2
    hs = float(sheet.lagerbock_abstand) / 2
    for sx in SIGNS:
        for sy in SIGNS:
            y0 = hs + 0.2 if sy > 0 else -hs - 4.2
            w = Part.makeCylinder(r, 4, App.Vector(sx * wb / 2, y0, -ad), App.Vector(0, 1, 0))
            v = s.common(w).Volume
            assert v < 1e-3, f"{label}: wheel {sx},{sy} collides with frame ({v:.2f} mm3)"
    # middle module screws: pilot hole open, material around it
    mx, my = float(sheet.mitte_x), float(sheet.breite) / 2 - 5.5
    for sy in SIGNS:
        rod = Part.makeCylinder(0.5, 6, App.Vector(mx, sy * my, -6.5))
        assert s.common(rod).Volume < 1e-3, f"{label}: middle pilot hole {sy} blocked"
        for dx, dy in ((2, 0), (-2, 0), (0, -sy * 2)):
            assert s.isInside(App.Vector(mx + dx, sy * my + dy, -4), 1e-3, True), \
                f"{label}: no material around middle screw {sy}"
    # shot module: one solid, pilot holes open from below, closed on top,
    # pocket diameter right, frame holes below them go through
    ss = shot.Shape
    assert ss.isValid() and len(ss.Solids) == 1, f"{label}: ModuleShot invalid or split"
    L, B = float(sheet.laenge), float(sheet.breite)
    st, sh = float(sheet.shot_schraube_tiefe), float(sheet.shot_hoehe)
    for px in (L / 2 - 5.5, -(L / 2 - 5.5), float(sheet.mitte_x)):
        for sy in SIGNS:
            py = sy * (B / 2 - 5.5)
            rod = Part.makeCylinder(0.5, st - 0.2, App.Vector(px, py, 0))
            assert ss.common(rod).Volume < 1e-3, f"{label}: shot pilot {px:.1f},{py:.1f} blocked"
            assert ss.isInside(App.Vector(px, py, sh - 0.3), 1e-3, True), f"{label}: shot pilot {px:.1f},{py:.1f} visible on top"
            assert s.common(Part.makeCylinder(0.5, 8.4, App.Vector(px, py, -8.2))).Volume < 1e-3, \
                f"{label}: frame hole below shot pilot {px:.1f},{py:.1f} not through"
    zr = float(sheet.glas_d_hoehe)
    want = (float(sheet.glas_d) + float(sheet.glas_spiel)) / 2
    for gx in (0, float(sheet.glas_abstand)):
        assert not ss.isInside(App.Vector(gx + want - 0.05, 0, zr), 1e-3, True) and \
               ss.isInside(App.Vector(gx + want + 0.05, 0, zr), 1e-3, True), f"{label}: pocket {gx} wrong size"
    # drains: open through the floor, and nothing of the frame or wheels below
    dx = float(sheet.glas_abstand) - float(sheet.abfluss_versatz)
    dr = float(sheet.abfluss_d) / 2
    dm = float(doc.getObject("ShotAbflussM").Radius)
    for gx, rr in ((0, dm), (dx, dr), (-dx, dr)):
        rod = Part.makeCylinder(rr, 60, App.Vector(gx, 0, -40))
        assert ss.common(rod).Volume < 1e-3, f"{label}: shot drain at x={gx:.1f} blocked"
        assert s.common(rod).Volume < 1e-3, f"{label}: frame below shot drain at x={gx:.1f}"
        for sx in SIGNS:
            for sy in SIGNS:
                y0 = hs + 0.2 if sy > 0 else -hs - 4.2
                w = Part.makeCylinder(r, 4, App.Vector(sx * wb / 2, y0, -ad), App.Vector(0, 1, 0))
                assert w.common(rod).Volume < 1e-3, f"{label}: wheel below shot drain at x={gx:.1f}"
    # coal module: single valid solids, lid fits, slots open and climbing inward
    for o in (coal_plain, coal_deco, lid):
        assert o.Shape.isValid(), f"{label}: {o.Name} invalid"
        assert len(o.Shape.Solids) == 1, f"{label}: {o.Name} has {len(o.Shape.Solids)} solids"
    for o in (coal2, lid2):
        assert o.Shape.isValid(), f"{label}: {o.Name} invalid"
        assert len(o.Shape.Solids) == 1, f"{label}: {o.Name} has {len(o.Shape.Solids)} solids"
        bb = o.Shape.BoundBox
        assert bb.XLength <= float(sheet.laenge) + 1e-6 and bb.YLength <= float(sheet.breite) + 1e-6, \
            f"{label}: {o.Name} sticks out of the frame footprint ({bb.XLength:.2f} x {bb.YLength:.2f})"
    v = coal2.Shape.common(lid2.Shape).Volume
    assert v < 1e-3, f"{label}: lid2 collides with ModuleCoal2 ({v:.2f} mm3)"
    for o in (coal_plain, coal_deco):
        v = o.Shape.common(lid.Shape).Volume
        assert v < 1e-3, f"{label}: lid collides with {o.Name} ({v:.2f} mm3)"
    # lid underside inside the walls must be one flat plane at TOP
    zt = float(sheet.modulplatte_dicke) + float(sheet.wanne_hoehe)
    assert abs(lid.Shape.BoundBox.ZMin - (zt - 0.0)) < 1e-6 or lid.Shape.BoundBox.ZMin >= zt - 1e-6, \
        f"{label}: lid reaches below its underside ({lid.Shape.BoundBox.ZMin:.2f} < {zt})"
    for sl in (slots if FULL else []):
        bb = sl.Base.Shape.BoundBox
        c = bb.Center
        d = App.Vector(sl.Dir)
        rod = Part.makeCylinder(0.2, d.Length, c, d)
        for o in (coal_plain, coal_deco):
            assert o.Shape.common(rod).Volume < 1e-4, f"{label}: {sl.Name} blocked in {o.Name}"
        assert d.z > 0, f"{label}: {sl.Name} does not climb inward"
    cm = coal_obj.Mesh
    zt_ = float(sheet.modulplatte_dicke) + float(sheet.wanne_hoehe)
    assert abs(coal_obj.Placement.Base.z - zt_) < 1e-6, f"{label}: coal not on the lid (z={coal_obj.Placement.Base.z})"
    assert cm.isSolid() and not cm.hasNonManifolds() and cm.Volume > 0, f"{label}: coal mesh not a clean solid"
    print(f"[ok] {label}: valid, 1 solid, "
          f"bbox {s.BoundBox.XLength:.1f} x {s.BoundBox.YLength:.1f} x {s.BoundBox.ZLength:.1f} mm, "
          f"volume {s.Volume/1000:.1f} cm3")

check("defaults 150x55")

if FULL:
    # parametric sanity check: resize, recompute, restore
    sheet.set(sheet.getCellFromAlias("laenge"), "220")
    sheet.set(sheet.getCellFromAlias("breite"), "45")
    doc.recompute()
    check("resized 220x45")

    sheet.set(sheet.getCellFromAlias("laenge"), "150")
    sheet.set(sheet.getCellFromAlias("breite"), "55")
    doc.recompute()
    check("restored 150x55")

# ---------------------------------------------------------------- outputs
# show the assembled wagon (frame + coal module 2 + its lid + coal), hide the rest
SHOWN = (final, coal2, lid2, coal_obj)
for o in doc.Objects:
    if hasattr(o, "Visibility"):
        o.Visibility = o in SHOWN

doc.saveAs(os.path.join(OUT, "WagonBase.FCStd"))

for obj, fname in ((final, "WagonBase.stl"), (plate_final, "ModulePlateBlank.stl"),
                   (coal_plain, "ModuleCoal.stl"), (coal_deco, "ModuleCoalDecorated.stl"),
                   (shot, "ModuleShot.stl"), (coal2, "ModuleCoal2.stl")):
    m = MeshPart.meshFromShape(Shape=obj.Shape, LinearDeflection=0.1, AngularDeflection=0.3)
    m.write(os.path.join(OUT, fname))
# lid = lid frame + coal load in one file (two overlapping shells; slicers merge them)
m = MeshPart.meshFromShape(Shape=lid.Shape, LinearDeflection=0.1, AngularDeflection=0.3)
coal_world = coal_obj.Mesh.copy()          # bake the feature placement into the points
coal_world.Placement = App.Placement()
coal_world.transformGeometry(coal_obj.Placement.toMatrix())
m.addMesh(coal_world)
m.write(os.path.join(OUT, "ModuleCoalLid.stl"))
m = MeshPart.meshFromShape(Shape=lid2.Shape, LinearDeflection=0.1, AngularDeflection=0.3)
m.addMesh(coal_world)
m.write(os.path.join(OUT, "ModuleCoal2Lid.stl"))

Part.export([final, plate_final, coal_plain, coal_deco, lid, shot, coal2, lid2], os.path.join(OUT, "WagonBase.step"))
print("[ok] wrote WagonBase.FCStd / .step / .stl files to", OUT)

# FreeCAD 1.1 can segfault while freeing Mesh features at shutdown; all
# files are written at this point, so leave without the teardown.
import sys
sys.stdout.flush(); sys.stderr.flush()
os._exit(0)
