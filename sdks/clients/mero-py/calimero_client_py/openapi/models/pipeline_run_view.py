from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.pipeline_run_view_state import PipelineRunViewState
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.pipeline_run_cost import PipelineRunCost
    from ..models.pipeline_caller_type_5 import PipelineCallerType5
    from ..models.pipeline_run_view_modules import PipelineRunViewModules
    from ..models.pipeline_caller_type_2 import PipelineCallerType2
    from ..models.pipeline_run_view_inputs import PipelineRunViewInputs
    from ..models.pipeline_caller_type_1 import PipelineCallerType1
    from ..models.pipeline_run_view_steps import PipelineRunViewSteps
    from ..models.pipeline_caller_type_0 import PipelineCallerType0
    from ..models.pipeline_caller_type_4 import PipelineCallerType4
    from ..models.pipeline_run_view_limits import PipelineRunViewLimits
    from ..models.pipeline_run_view_input_files import PipelineRunViewInputFiles
    from ..models.pipeline_run_view_params import PipelineRunViewParams
    from ..models.pipeline_caller_type_3 import PipelineCallerType3


T = TypeVar("T", bound="PipelineRunView")


@_attrs_define
class PipelineRunView:
    """A pipeline run as the app's leader shows it: its stored record (`PipelineRun`), keyed by `id`, reshaped for the
    reader (results and file hashes only to those who handle the app's files). `costSoFar` is a decimal string, under a
    cost cap.

        Attributes:
            id (str):
            pipeline (str):
            version (str):
            hash_ (str):
            compute_bundle (str):
            owner (str):
            inputs (PipelineRunViewInputs):
            params (PipelineRunViewParams):
            caller (Union['PipelineCallerType0', 'PipelineCallerType1', 'PipelineCallerType2', 'PipelineCallerType3',
                'PipelineCallerType4', 'PipelineCallerType5']): Who started a pipeline run.
            payer (str):
            start_ts_ms (int):
            state (PipelineRunViewState):
            cancel_requested (bool):
            fail_fast (bool):
            stopping (bool):
            steps (PipelineRunViewSteps):
            end_ts_ms (Union[None, int]):
            modules (Union[Unset, PipelineRunViewModules]):
            input_files (Union[Unset, PipelineRunViewInputFiles]):
            cost (Union[Unset, PipelineRunCost]): The cost cap of a pipeline run and the price table it was admitted under;
                the figures that can pass 2^53 are decimal strings.
            cost_so_far (Union[Unset, str]): an unsigned 64-bit integer as a decimal string (D20)
            retry_of (Union[Unset, str]):
            limits (Union[Unset, PipelineRunViewLimits]):
    """

    id: str
    pipeline: str
    version: str
    hash_: str
    compute_bundle: str
    owner: str
    inputs: "PipelineRunViewInputs"
    params: "PipelineRunViewParams"
    caller: Union[
        "PipelineCallerType0",
        "PipelineCallerType1",
        "PipelineCallerType2",
        "PipelineCallerType3",
        "PipelineCallerType4",
        "PipelineCallerType5",
    ]
    payer: str
    start_ts_ms: int
    state: PipelineRunViewState
    cancel_requested: bool
    fail_fast: bool
    stopping: bool
    steps: "PipelineRunViewSteps"
    end_ts_ms: Union[None, int]
    modules: Union[Unset, "PipelineRunViewModules"] = UNSET
    input_files: Union[Unset, "PipelineRunViewInputFiles"] = UNSET
    cost: Union[Unset, "PipelineRunCost"] = UNSET
    cost_so_far: Union[Unset, str] = UNSET
    retry_of: Union[Unset, str] = UNSET
    limits: Union[Unset, "PipelineRunViewLimits"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipeline_run_cost import PipelineRunCost
        from ..models.pipeline_caller_type_5 import PipelineCallerType5
        from ..models.pipeline_run_view_modules import PipelineRunViewModules
        from ..models.pipeline_caller_type_2 import PipelineCallerType2
        from ..models.pipeline_run_view_inputs import PipelineRunViewInputs
        from ..models.pipeline_caller_type_1 import PipelineCallerType1
        from ..models.pipeline_run_view_steps import PipelineRunViewSteps
        from ..models.pipeline_caller_type_0 import PipelineCallerType0
        from ..models.pipeline_caller_type_4 import PipelineCallerType4
        from ..models.pipeline_run_view_limits import PipelineRunViewLimits
        from ..models.pipeline_run_view_input_files import PipelineRunViewInputFiles
        from ..models.pipeline_run_view_params import PipelineRunViewParams
        from ..models.pipeline_caller_type_3 import PipelineCallerType3

        id = self.id

        pipeline = self.pipeline

        version = self.version

        hash_ = self.hash_

        compute_bundle = self.compute_bundle

        owner = self.owner

        inputs = self.inputs.to_dict()

        params = self.params.to_dict()

        caller: dict[str, Any]
        if isinstance(self.caller, PipelineCallerType0):
            caller = self.caller.to_dict()
        elif isinstance(self.caller, PipelineCallerType1):
            caller = self.caller.to_dict()
        elif isinstance(self.caller, PipelineCallerType2):
            caller = self.caller.to_dict()
        elif isinstance(self.caller, PipelineCallerType3):
            caller = self.caller.to_dict()
        elif isinstance(self.caller, PipelineCallerType4):
            caller = self.caller.to_dict()
        else:
            caller = self.caller.to_dict()

        payer = self.payer

        start_ts_ms = self.start_ts_ms

        state = self.state.value

        cancel_requested = self.cancel_requested

        fail_fast = self.fail_fast

        stopping = self.stopping

        steps = self.steps.to_dict()

        end_ts_ms: Union[None, int]
        end_ts_ms = self.end_ts_ms

        modules: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.modules, Unset):
            modules = self.modules.to_dict()

        input_files: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.input_files, Unset):
            input_files = self.input_files.to_dict()

        cost: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.cost, Unset):
            cost = self.cost.to_dict()

        cost_so_far = self.cost_so_far

        retry_of = self.retry_of

        limits: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.limits, Unset):
            limits = self.limits.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "pipeline": pipeline,
                "version": version,
                "hash": hash_,
                "computeBundle": compute_bundle,
                "owner": owner,
                "inputs": inputs,
                "params": params,
                "caller": caller,
                "payer": payer,
                "startTsMs": start_ts_ms,
                "state": state,
                "cancelRequested": cancel_requested,
                "failFast": fail_fast,
                "stopping": stopping,
                "steps": steps,
                "endTsMs": end_ts_ms,
            }
        )
        if modules is not UNSET:
            field_dict["modules"] = modules
        if input_files is not UNSET:
            field_dict["inputFiles"] = input_files
        if cost is not UNSET:
            field_dict["cost"] = cost
        if cost_so_far is not UNSET:
            field_dict["costSoFar"] = cost_so_far
        if retry_of is not UNSET:
            field_dict["retryOf"] = retry_of
        if limits is not UNSET:
            field_dict["limits"] = limits

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipeline_run_cost import PipelineRunCost
        from ..models.pipeline_caller_type_5 import PipelineCallerType5
        from ..models.pipeline_run_view_modules import PipelineRunViewModules
        from ..models.pipeline_caller_type_2 import PipelineCallerType2
        from ..models.pipeline_run_view_inputs import PipelineRunViewInputs
        from ..models.pipeline_caller_type_1 import PipelineCallerType1
        from ..models.pipeline_run_view_steps import PipelineRunViewSteps
        from ..models.pipeline_caller_type_0 import PipelineCallerType0
        from ..models.pipeline_caller_type_4 import PipelineCallerType4
        from ..models.pipeline_run_view_limits import PipelineRunViewLimits
        from ..models.pipeline_run_view_input_files import PipelineRunViewInputFiles
        from ..models.pipeline_run_view_params import PipelineRunViewParams
        from ..models.pipeline_caller_type_3 import PipelineCallerType3

        d = dict(src_dict)
        id = d.pop("id")

        pipeline = d.pop("pipeline")

        version = d.pop("version")

        hash_ = d.pop("hash")

        compute_bundle = d.pop("computeBundle")

        owner = d.pop("owner")

        inputs = PipelineRunViewInputs.from_dict(d.pop("inputs"))

        params = PipelineRunViewParams.from_dict(d.pop("params"))

        def _parse_caller(
            data: object,
        ) -> Union[
            "PipelineCallerType0",
            "PipelineCallerType1",
            "PipelineCallerType2",
            "PipelineCallerType3",
            "PipelineCallerType4",
            "PipelineCallerType5",
        ]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pipeline_caller_type_0 = (
                    PipelineCallerType0.from_dict(data)
                )

                return componentsschemas_pipeline_caller_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pipeline_caller_type_1 = (
                    PipelineCallerType1.from_dict(data)
                )

                return componentsschemas_pipeline_caller_type_1
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pipeline_caller_type_2 = (
                    PipelineCallerType2.from_dict(data)
                )

                return componentsschemas_pipeline_caller_type_2
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pipeline_caller_type_3 = (
                    PipelineCallerType3.from_dict(data)
                )

                return componentsschemas_pipeline_caller_type_3
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pipeline_caller_type_4 = (
                    PipelineCallerType4.from_dict(data)
                )

                return componentsschemas_pipeline_caller_type_4
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_pipeline_caller_type_5 = PipelineCallerType5.from_dict(
                data
            )

            return componentsschemas_pipeline_caller_type_5

        caller = _parse_caller(d.pop("caller"))

        payer = d.pop("payer")

        start_ts_ms = d.pop("startTsMs")

        state = PipelineRunViewState(d.pop("state"))

        cancel_requested = d.pop("cancelRequested")

        fail_fast = d.pop("failFast")

        stopping = d.pop("stopping")

        steps = PipelineRunViewSteps.from_dict(d.pop("steps"))

        def _parse_end_ts_ms(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        end_ts_ms = _parse_end_ts_ms(d.pop("endTsMs"))

        _modules = d.pop("modules", UNSET)
        modules: Union[Unset, PipelineRunViewModules]
        if isinstance(_modules, Unset):
            modules = UNSET
        else:
            modules = PipelineRunViewModules.from_dict(_modules)

        _input_files = d.pop("inputFiles", UNSET)
        input_files: Union[Unset, PipelineRunViewInputFiles]
        if isinstance(_input_files, Unset):
            input_files = UNSET
        else:
            input_files = PipelineRunViewInputFiles.from_dict(_input_files)

        _cost = d.pop("cost", UNSET)
        cost: Union[Unset, PipelineRunCost]
        if isinstance(_cost, Unset):
            cost = UNSET
        else:
            cost = PipelineRunCost.from_dict(_cost)

        cost_so_far = d.pop("costSoFar", UNSET)

        retry_of = d.pop("retryOf", UNSET)

        _limits = d.pop("limits", UNSET)
        limits: Union[Unset, PipelineRunViewLimits]
        if isinstance(_limits, Unset):
            limits = UNSET
        else:
            limits = PipelineRunViewLimits.from_dict(_limits)

        pipeline_run_view = cls(
            id=id,
            pipeline=pipeline,
            version=version,
            hash_=hash_,
            compute_bundle=compute_bundle,
            owner=owner,
            inputs=inputs,
            params=params,
            caller=caller,
            payer=payer,
            start_ts_ms=start_ts_ms,
            state=state,
            cancel_requested=cancel_requested,
            fail_fast=fail_fast,
            stopping=stopping,
            steps=steps,
            end_ts_ms=end_ts_ms,
            modules=modules,
            input_files=input_files,
            cost=cost,
            cost_so_far=cost_so_far,
            retry_of=retry_of,
            limits=limits,
        )

        pipeline_run_view.additional_properties = d
        return pipeline_run_view

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
