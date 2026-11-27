from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.pipeline_run_cost_table import PipelineRunCostTable


T = TypeVar("T", bound="PipelineRunCost")


@_attrs_define
class PipelineRunCost:
    """The cost cap of a pipeline run and the price table it was admitted under; the figures that can pass 2^53 are decimal
    strings.

        Attributes:
            max_cost (str): an unsigned 64-bit integer as a decimal string, exact past 2^53
            price_version (int):
            table (PipelineRunCostTable):
            worst_case (str): an unsigned 64-bit integer as a decimal string, exact past 2^53
    """

    max_cost: str
    price_version: int
    table: "PipelineRunCostTable"
    worst_case: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipeline_run_cost_table import PipelineRunCostTable

        max_cost = self.max_cost

        price_version = self.price_version

        table = self.table.to_dict()

        worst_case = self.worst_case

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "maxCost": max_cost,
                "priceVersion": price_version,
                "table": table,
                "worstCase": worst_case,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipeline_run_cost_table import PipelineRunCostTable

        d = dict(src_dict)
        max_cost = d.pop("maxCost")

        price_version = d.pop("priceVersion")

        table = PipelineRunCostTable.from_dict(d.pop("table"))

        worst_case = d.pop("worstCase")

        pipeline_run_cost = cls(
            max_cost=max_cost,
            price_version=price_version,
            table=table,
            worst_case=worst_case,
        )

        pipeline_run_cost.additional_properties = d
        return pipeline_run_cost

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
