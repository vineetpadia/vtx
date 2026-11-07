//! Measurements use GPU-resolved surface coordinates, never pixel distances.
use super::{
    units::{length_factor, Units},
    Result,
};
use nalgebra_glm::{DMat3, DVec3};
use serde_json::{json, Value};

#[derive(Debug)]
pub enum Request {
    Distance([usize; 2]),
    Angle([usize; 3]),
    Diameter(Vec<usize>),
}
impl Request {
    pub fn parse(kind: &str, value: &str) -> Result<Self> {
        let indices: Vec<usize> = value
            .split(',')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        match kind {
            "--distance" if indices.len()==2=>Ok(Self::Distance([indices[0],indices[1]])),
            "--angle" if indices.len()==3=>Ok(Self::Angle([indices[0],indices[1],indices[2]])),
            "--diameter" if (3..=256).contains(&indices.len())=>{
                if indices.iter().collect::<std::collections::HashSet<_>>().len()!=indices.len(){return Err("Diameter query indices must be distinct".into());}
                Ok(Self::Diameter(indices))
            },
            _=>Err("Distance needs two query indices; angle needs three (middle is vertex); diameter needs 3..256 distinct indices".into()),
        }
    }
    pub fn indices(&self) -> &[usize] {
        match self {
            Self::Distance(v) => v,
            Self::Angle(v) => v,
            Self::Diameter(v) => v,
        }
    }
    fn kind(&self) -> &str {
        match self {
            Self::Distance(_) => "distance",
            Self::Angle(_) => "angle",
            Self::Diameter(_) => "diameter",
        }
    }
}

pub fn validate_unit(units: &Units, requested: Option<&str>) -> Result<()> {
    if let Some(label) = requested {
        if length_factor(label).is_none() {
            return Err("Measurement unit must be mm, cm, m, in, or ft".into());
        }
        if units.meters_per_model_unit.is_none() {
            return Err("Cannot convert unknown/ambiguous model units. Supply a verified --unit override or omit --measure-unit.".into());
        }
    }
    Ok(())
}

pub fn evaluate(
    requests: &[Request],
    queries: &[Value],
    units: &Units,
    requested: Option<&str>,
) -> Vec<Value> {
    requests.iter().map(|request| {
        let mut result=json!({"type":request.kind(),"queries":request.indices(),"basis":"imported surface mesh (float32), not exact CAD topology","unit_provenance":units.source});
        let points:Option<Vec<_>>=request.indices().iter().map(|&i| {
            let q=queries.get(i)?;
            if q["hit"]!=true{return None;}
            let p=q["position"].as_array()?;
            if p.len()!=3{return None;}
            let v=DVec3::new(p[0].as_f64()?,p[1].as_f64()?,p[2].as_f64()?);
            v.iter().all(|v|v.is_finite()).then_some(v)
        }).collect();
        let Some(points)=points else {result["status"]=json!("unavailable");result["reason"]=json!("At least one selected query missed visible geometry or has no finite position.");return result;};
        result["points"]=json!(points.iter().map(|p|[p.x,p.y,p.z]).collect::<Vec<_>>());
        match request {
            Request::Distance(_)=>{
                let distance=(points[1]-points[0]).norm();
                result["status"]=json!("ok");result["method"]=json!("3D Euclidean point-to-point distance");
                length(&mut result,distance,units,requested);
            },
            Request::Angle(_)=>{
                let a=points[0]-points[1];let b=points[2]-points[1];
                if a.norm()==0.0||b.norm()==0.0 {result["status"]=json!("unavailable");result["reason"]=json!("Angle has a zero-length leg.");}
                else {result["status"]=json!("ok");result["degrees"]=json!((a.dot(&b)/(a.norm()*b.norm())).clamp(-1.0,1.0).acos().to_degrees());result["method"]=json!("3D included angle; middle selected point is vertex");}
            },
            Request::Diameter(_)=>match circle(&points) {
                Err(reason)=>{result["status"]=json!("rejected_fit");result["reason"]=json!(reason);},
                Ok(fit)=>{
                    result["method"]=json!("Least-squares planar circle fit to user-selected surface points");
                    result["center"]=json!([fit.center.x,fit.center.y,fit.center.z]);
                    result["plane_normal"]=json!([fit.normal.x,fit.normal.y,fit.normal.z]);
                    result["fit"]=json!({"radial_max_residual_model_units":fit.radial,"planarity_max_residual_model_units":fit.planarity,
                        "acceptance_tolerance_model_units":fit.radius*0.01,"angular_coverage_degrees":fit.coverage,
                        "independent_validation":points.len()>3,"note":"Residual threshold is 1% of fitted radius. Three points construct a circle but cannot independently validate circularity. Point selection does not establish that these points bound a hole or cylindrical feature."});
                    if fit.radial>fit.radius*0.01||fit.planarity>fit.radius*0.01||fit.coverage<90.0 {
                        result["status"]=json!("rejected_fit");result["reason"]=json!("Points are insufficiently circular/coplanar (1% radius tolerance), or span less than 90 degrees. No diameter is reported.");
                    } else {
                        result["status"]=json!(if points.len()==3{"constructed_circle"}else{"ok"});
                        length(&mut result,fit.radius*2.0,units,requested);
                    }
                }
            },
        } result
    }).collect()
}
fn length(result: &mut Value, value: f64, units: &Units, requested: Option<&str>) {
    let factor = units.meters_per_model_unit;
    let (display, label) = if let Some(unit) = requested {
        (value * factor.unwrap() / length_factor(unit).unwrap(), unit)
    } else {
        (value, units.label.as_str())
    };
    result["value"] = json!(display);
    result["unit"] = json!(label);
    result["value_model_units"] = json!(value);
    result["value_meters"] = json!(factor.map(|f| f * value));
}
struct Circle {
    center: DVec3,
    normal: DVec3,
    radius: f64,
    radial: f64,
    planarity: f64,
    coverage: f64,
}
fn circle(points: &[DVec3]) -> std::result::Result<Circle, &'static str> {
    let origin = points.iter().copied().sum::<DVec3>() / points.len() as f64;
    let scale = points
        .iter()
        .map(|p| (*p - origin).norm())
        .fold(0.0, f64::max);
    if scale <= 0.0 {
        return Err("Coincident points do not define a circle.");
    }
    let points: Vec<_> = points.iter().map(|p| (*p - origin) / scale).collect();
    let mut pair = (0, 0);
    let mut max = 0.0;
    for i in 0..points.len() {
        for j in i + 1..points.len() {
            let d = (points[j] - points[i]).norm_squared();
            if d > max {
                max = d;
                pair = (i, j);
            }
        }
    }
    let u = (points[pair.1] - points[pair.0]).normalize();
    let mut n = DVec3::zeros();
    for p in &points {
        let cross = u.cross(&(*p - points[pair.0]));
        if cross.norm_squared() > n.norm_squared() {
            n = cross;
        }
    }
    if n.norm() < 1e-6 {
        return Err("Collinear or nearly collinear points do not define a reliable circle.");
    }
    let n = n.normalize();
    let v = n.cross(&u);
    let mut matrix = DMat3::zeros();
    let mut rhs = DVec3::zeros();
    for p in &points {
        let x = p.dot(&u);
        let y = p.dot(&v);
        let row = DVec3::new(2.0 * x, 2.0 * y, 1.0);
        matrix += row * row.transpose();
        rhs += row * (x * x + y * y);
    }
    if matrix.determinant().abs() < 1e-10 {
        return Err("Circle fit is ill-conditioned.");
    }
    let fit = matrix.try_inverse().ok_or("Circle fit is singular.")? * rhs;
    let radius = (fit.z + fit.x * fit.x + fit.y * fit.y).sqrt();
    if !radius.is_finite() || radius <= 0.0 {
        return Err("Circle fit has no finite positive radius.");
    }
    let center = u * fit.x + v * fit.y;
    let mut radial: f64 = 0.0;
    let mut planarity: f64 = 0.0;
    let mut angles = Vec::new();
    for p in &points {
        let x = p.dot(&u) - fit.x;
        let y = p.dot(&v) - fit.y;
        radial = radial.max((x.hypot(y) - radius).abs());
        planarity = planarity.max(p.dot(&n).abs());
        angles.push(y.atan2(x));
    }
    angles.sort_by(f64::total_cmp);
    let mut gap = angles[0] + std::f64::consts::TAU - angles[angles.len() - 1];
    for pair in angles.windows(2) {
        gap = gap.max(pair[1] - pair[0]);
    }
    Ok(Circle {
        center: origin + center * scale,
        normal: n,
        radius: radius * scale,
        radial: radial * scale,
        planarity: planarity * scale,
        coverage: (std::f64::consts::TAU - gap).to_degrees(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(p: [f64; 3]) -> Value {
        json!({"hit":true,"position":p})
    }
    #[test]
    fn distances_convert_without_changing_points() {
        let u = Units::known("in", 0.0254, "test", Value::Null);
        let q = [hit([0., 0., 0.]), hit([3., 4., 0.])];
        let result = evaluate(&[Request::Distance([0, 1])], &q, &u, Some("mm"));
        assert_eq!(result[0]["value_model_units"], 5.0);
        assert!((result[0]["value"].as_f64().unwrap() - 127.0).abs() < 1e-10);
        assert!(validate_unit(&Units::unknown("unknown"), Some("mm")).is_err());
        let q = [hit([0., 0., 0.]), json!({"hit":false})];
        assert_eq!(
            evaluate(&[Request::Distance([0, 1])], &q, &u, None)[0]["status"],
            "unavailable"
        );
    }
    #[test]
    fn angles_and_degenerate_legs() {
        let u = Units::unknown("test");
        let q = [hit([1., 0., 0.]), hit([0., 0., 0.]), hit([0., 1., 0.])];
        assert_eq!(
            evaluate(&[Request::Angle([0, 1, 2])], &q, &u, None)[0]["degrees"],
            90.0
        );
        assert_eq!(
            evaluate(&[Request::Angle([0, 0, 2])], &q, &u, None)[0]["status"],
            "unavailable"
        );
    }
    #[test]
    fn tilted_circle_large_offset_and_small_units() {
        for radius in [1e-6, 5.0, 1e6] {
            let origin = DVec3::new(1000.0, 2000.0, 3000.0);
            let u = DVec3::new(1., 1., 0.).normalize();
            let v = DVec3::new(0., 0., 1.);
            let points: Vec<_> = (0..8)
                .map(|i| {
                    let t = i as f64 * std::f64::consts::TAU / 8.;
                    origin + radius * (t.cos() * u + t.sin() * v)
                })
                .collect();
            let fit = circle(&points).unwrap();
            assert!((fit.radius - radius).abs() < radius * 1e-5);
            assert!((fit.center - origin).norm() < radius * 1e-5);
        }
    }
    #[test]
    fn rejects_bad_fits_and_reports_three_point_construction() {
        let u = Units::unknown("test");
        let q = [hit([1., 0., 0.]), hit([0., 1., 0.]), hit([-1., 0., 0.])];
        let fit = evaluate(&[Request::Diameter(vec![0, 1, 2])], &q, &u, None);
        assert_eq!(fit[0]["status"], "constructed_circle");
        assert!((fit[0]["value"].as_f64().unwrap() - 2.).abs() < 1e-10);
        let line = [
            DVec3::new(0., 0., 0.),
            DVec3::new(1., 0., 0.),
            DVec3::new(2., 0., 0.),
        ];
        assert!(circle(&line).is_err());
        let q = [
            hit([1., 0., 0.]),
            hit([0., 1., 0.]),
            hit([-1., 0., 0.]),
            hit([0., -3., 2.]),
        ];
        assert_eq!(
            evaluate(&[Request::Diameter(vec![0, 1, 2, 3])], &q, &u, None)[0]["status"],
            "rejected_fit"
        );
    }
}
