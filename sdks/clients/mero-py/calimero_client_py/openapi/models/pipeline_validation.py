from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.pipeline_error import PipelineError


T = TypeVar("T", bound="PipelineValidation")


@_attrs_define
class PipelineValidation:
    """`POST /pipelines/validate`: whether the pipeline compiles (and, against a release, checks), every error, its
    canonical JSON, and its worst-case fuel and cost (decimal strings).

        Attributes:
            ok (bool):
            errors (list['PipelineError']):
            pipeline (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for
                whose it is).
            worst_case_fuel (Union[Unset, str]):
            worst_case_cost (Union[Unset, str]):
    """

    ok: bool
    errors: list["PipelineError"]
    pipeline: Union[Unset, Any] = UNSET
    worst_case_fuel: Union[Unset, str] = UNSET
    worst_case_cost: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipeline_error import PipelineError

        ok = self.ok

        errors = []
        for errors_item_data in self.errors:
            errors_item = errors_item_data.to_dict()
            errors.append(errors_item)

        pipeline = self.pipeline

        worst_case_fuel = self.worst_case_fuel

        worst_case_cost = self.worst_case_cost

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "ok": ok,
                "errors": errors,
            }
        )
        if pipeline is not UNSET:
            field_dict["pipeline"] = pipeline
        if worst_case_fuel is not UNSET:
            field_dict["worstCaseFuel"] = worst_case_fuel
        if worst_case_cost is not UNSET:
            field_dict["worstCaseCost"] = worst_case_cost

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipeline_error import PipelineError

        d = dict(src_dict)
        ok = d.pop("ok")

        errors = []
        _errors = d.pop("errors")
        for errors_item_data in _errors:
            errors_item = PipelineError.from_dict(errors_item_data)

            errors.append(errors_item)

        pipeline = d.pop("pipeline", UNSET)

        worst_case_fuel = d.pop("worstCaseFuel", UNSET)

        worst_case_cost = d.pop("worstCaseCost", UNSET)

        pipeline_validation = cls(
            ok=ok,
            errors=errors,
            pipeline=pipeline,
            worst_case_fuel=worst_case_fuel,
            worst_case_cost=worst_case_cost,
        )

        pipeline_validation.additional_properties = d
        return pipeline_validation

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
