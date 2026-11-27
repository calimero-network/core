from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.pipeline_run_step_status import PipelineRunStepStatus
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.pipeline_run_step_fan import PipelineRunStepFan
    from ..models.pipeline_run_output import PipelineRunOutput
    from ..models.pipeline_run_step_cached_keys import PipelineRunStepCachedKeys


T = TypeVar("T", bound="PipelineRunStep")


@_attrs_define
class PipelineRunStep:
    """One step of a pipeline run. `runs`, `outputs`, `result` and `results` come only with one run (`GET
    /apps/{app}/pipeline-runs/{run}`), not in the list; `result` and `results` are `null` to those who may not handle
    the app's files. `cost` is a decimal string, under a cost cap.

        Attributes:
            status (PipelineRunStepStatus):
            results_too_large (bool):
            reason (Union[None, str]):
            fan (PipelineRunStepFan):
            finished (int):
            failed_items (int):
            cached_items (int):
            cached (bool):
            runs (Union[Unset, list[str]]):
            cached_count (Union[Unset, int]):
            cached_keys (Union[Unset, PipelineRunStepCachedKeys]):
            cost (Union[Unset, str]): an unsigned 64-bit integer as a decimal string, exact past 2^53
            capped (Union[Unset, bool]):
            legacy_items (Union[Unset, int]):
            outputs (Union[Unset, list['PipelineRunOutput']]):
            result (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
                it is).
            results (Union[None, Unset, list[Any]]):
    """

    status: PipelineRunStepStatus
    results_too_large: bool
    reason: Union[None, str]
    fan: "PipelineRunStepFan"
    finished: int
    failed_items: int
    cached_items: int
    cached: bool
    runs: Union[Unset, list[str]] = UNSET
    cached_count: Union[Unset, int] = UNSET
    cached_keys: Union[Unset, "PipelineRunStepCachedKeys"] = UNSET
    cost: Union[Unset, str] = UNSET
    capped: Union[Unset, bool] = UNSET
    legacy_items: Union[Unset, int] = UNSET
    outputs: Union[Unset, list["PipelineRunOutput"]] = UNSET
    result: Union[Unset, Any] = UNSET
    results: Union[None, Unset, list[Any]] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipeline_run_step_fan import PipelineRunStepFan
        from ..models.pipeline_run_output import PipelineRunOutput
        from ..models.pipeline_run_step_cached_keys import PipelineRunStepCachedKeys

        status = self.status.value

        results_too_large = self.results_too_large

        reason: Union[None, str]
        reason = self.reason

        fan = self.fan.to_dict()

        finished = self.finished

        failed_items = self.failed_items

        cached_items = self.cached_items

        cached = self.cached

        runs: Union[Unset, list[str]] = UNSET
        if not isinstance(self.runs, Unset):
            runs = self.runs

        cached_count = self.cached_count

        cached_keys: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.cached_keys, Unset):
            cached_keys = self.cached_keys.to_dict()

        cost = self.cost

        capped = self.capped

        legacy_items = self.legacy_items

        outputs: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.outputs, Unset):
            outputs = []
            for outputs_item_data in self.outputs:
                outputs_item = outputs_item_data.to_dict()
                outputs.append(outputs_item)

        result = self.result

        results: Union[None, Unset, list[Any]]
        if isinstance(self.results, Unset):
            results = UNSET
        elif isinstance(self.results, list):
            results = self.results

        else:
            results = self.results

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "status": status,
                "resultsTooLarge": results_too_large,
                "reason": reason,
                "fan": fan,
                "finished": finished,
                "failedItems": failed_items,
                "cachedItems": cached_items,
                "cached": cached,
            }
        )
        if runs is not UNSET:
            field_dict["runs"] = runs
        if cached_count is not UNSET:
            field_dict["cachedCount"] = cached_count
        if cached_keys is not UNSET:
            field_dict["cachedKeys"] = cached_keys
        if cost is not UNSET:
            field_dict["cost"] = cost
        if capped is not UNSET:
            field_dict["capped"] = capped
        if legacy_items is not UNSET:
            field_dict["legacyItems"] = legacy_items
        if outputs is not UNSET:
            field_dict["outputs"] = outputs
        if result is not UNSET:
            field_dict["result"] = result
        if results is not UNSET:
            field_dict["results"] = results

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipeline_run_step_fan import PipelineRunStepFan
        from ..models.pipeline_run_output import PipelineRunOutput
        from ..models.pipeline_run_step_cached_keys import PipelineRunStepCachedKeys

        d = dict(src_dict)
        status = PipelineRunStepStatus(d.pop("status"))

        results_too_large = d.pop("resultsTooLarge")

        def _parse_reason(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        reason = _parse_reason(d.pop("reason"))

        fan = PipelineRunStepFan.from_dict(d.pop("fan"))

        finished = d.pop("finished")

        failed_items = d.pop("failedItems")

        cached_items = d.pop("cachedItems")

        cached = d.pop("cached")

        runs = cast(list[str], d.pop("runs", UNSET))

        cached_count = d.pop("cachedCount", UNSET)

        _cached_keys = d.pop("cachedKeys", UNSET)
        cached_keys: Union[Unset, PipelineRunStepCachedKeys]
        if isinstance(_cached_keys, Unset):
            cached_keys = UNSET
        else:
            cached_keys = PipelineRunStepCachedKeys.from_dict(_cached_keys)

        cost = d.pop("cost", UNSET)

        capped = d.pop("capped", UNSET)

        legacy_items = d.pop("legacyItems", UNSET)

        outputs = []
        _outputs = d.pop("outputs", UNSET)
        for outputs_item_data in _outputs or []:
            outputs_item = PipelineRunOutput.from_dict(outputs_item_data)

            outputs.append(outputs_item)

        result = d.pop("result", UNSET)

        def _parse_results(data: object) -> Union[None, Unset, list[Any]]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                results_type_0 = cast(list[Any], data)

                return results_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, list[Any]], data)

        results = _parse_results(d.pop("results", UNSET))

        pipeline_run_step = cls(
            status=status,
            results_too_large=results_too_large,
            reason=reason,
            fan=fan,
            finished=finished,
            failed_items=failed_items,
            cached_items=cached_items,
            cached=cached,
            runs=runs,
            cached_count=cached_count,
            cached_keys=cached_keys,
            cost=cost,
            capped=capped,
            legacy_items=legacy_items,
            outputs=outputs,
            result=result,
            results=results,
        )

        pipeline_run_step.additional_properties = d
        return pipeline_run_step

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
